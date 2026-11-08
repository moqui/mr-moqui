# Profiling

Status 2026-10-21: four rounds of profiling and optimization of entity, service, data load and XML/definition work against embedded Postgres, built and committed. Round 5 tested and measured against an external Postgres 18 in Docker ([Round 5](#round-5-external-postgres)), fixed what it found, and sends a data load's rows several to a statement ([Round 6](#round-6-multi-row-data-load-statements)). Round 7, on an M1 Pro (macOS), measured COPY and dropped it, added a multi-threaded benchmark and fixed the lock it showed, and cut what a transaction keeps per write ([Round 7](#round-7-macos-threads-copy-bookkeeping)); what is left leads the [Next steps](#next-steps). Round 10 measured QuickJS and its bindings with a new `js_speed` test and ranked what the profiles show ([Round 10](#round-10-javascript)); Round 11 fixed the five binding costs it found ([Round 11](#round-11-javascript-bindings)), Rounds 12 and 13 the per-row work of the html and qvt form-lists ([Round 12](#round-12-form-list-rows), [Round 13](#round-13-row-frames-and-qvt-row-values)). Companion to [Performance-Summary](Performance-Summary.md), which digests the performance plan; this document records what was measured and changed, how to measure, and what is left.

## How to measure

Seven opt-in speed tests in `crates/moqui-host/tests/` (each `test = false` and `#[ignore]`, so a default `cargo test` neither links nor runs them):

| Test | What it times | Needs |
|---|---|---|
| `entity_speed` | entity facade finds (cache, PK, list, count, iterate 100 rows) and writes (create, update, delete, `create_or_update`), in one transaction and in autocommit; then a cached find, a PK find and writes on 1, 2, 4 and 8 threads at once (`ENTITY_SPEED_THREADS`; `ENTITY_SPEED_THREADS_ONLY` runs that section alone) | embedded Postgres |
| `service_speed` | the `Tools/Database/SpeedTest.xml` calls: entity-auto create/update/delete of `moqui.test.TestEntity` and `echo#Data`, plus the same entity calls direct, in one transaction and each in its own | embedded Postgres |
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

## Round 14: the transaction cache

[TX-Cache](TX-Cache.md) Phases 0 to 5 made the transaction cache correct and fast enough to turn on, and turned it on for five framework services and parts of five pieces of framework logic. A new opt-in test, `tx_cache_speed`, times each with the cache off and on, one transaction a call; the harness has a waited round-trips column now (`rt`, from `driver::round_trips`, counted by the Postgres wire client when a read waits after a send). Numbers are per call, release build; "delay" is an external Postgres in docker with `tc netem delay 200us`, where a waited round trip costs about 0.3 ms.

| Call | rt off | rt on | ms off, delay | ms on, delay | ms off, local | ms on, local |
|---|---|---|---|---|---|---|
| S1 `heartbeat#UserStatus` | 8 | 5 | 2.26 | 1.56 | 0.30 | 0.27-0.32 |
| S2 `mark#Read` | 4 | 4 | 1.33 | 1.36 | 0.19 | 0.22 |
| S3 `send#Message` | 14 | 12 | 4.87 | 4.10 | 0.98 | 1.15 |
| S4 `update#WikiPage` | 117 | 17 | 33.5 | 9.85 | 5.2 | 5.9 |
| S5 `set#MessageFlags` | 21 | 19 | 4.70 | 4.20 | 1.13 | 1.24 |
| F1 notification persist, 3 users | 8 | 5 | 2.15 | 1.38 | 0.15 | |
| F4 audited update | 4 | 3 | 1.19 | 0.97 | 0.12 | 0.13 |
| F5 fed update | 14 | 13 | 2.40 | 2.25 | 0.94 | 1.06 |

"off" for F1 is the baseline (its persist is a `force-cache` frame now). Local times swing; the round trips do not.

With a network between the server and the database the cache wins wherever a call reads a row again or writes several: S4 reads its page and space over and over and goes from 117 waited round trips to 17. Over a local socket a call that reads little again is a little slower with the cache on (S3, S4, S5): what the cache does costs more than the round trips it saves there.

**Where the cache's own cost is** (heaptrack, allocations of the timed row only, `--flamegraph-cost-type allocations`; samply needs `perf_event_paranoid` at 1, which was 4 on this run, so no CPU profile yet). S4 with the cache on makes about 12,200 allocations a call (48,000 in the full run, where its page has more versions by then: `set_sequenced_id_secondary` and the wiki's resource reads grow with them, 17% and 29% of this row, and are the wiki's, not the cache's). Of the row:

| Share | What |
|---|---|
| 44% | everything under `overlay_rows`, the cache's read path |
| 16% | `load_plain`; of it 7% `plan_select`, the select SQL text built again on every read (the plain find path takes it from `sql_cache`) |
| 8% | `keep_list`, each row of a list find copied into an `Rc` to keep |
| 6% | `pinned_row`, a find by key; `load_by_pk` builds its SQL each time too |
| 3.5% | `owned_lookup_key`, the list key string built for every find |
| 2.6% | `values_of`, kept rows copied back out into `EntityValue`s |
| 2.2% | `pinned_keys` and `equalities`, a `HashMap` for every find |

S3's row (5,250 allocations a call) has the same first item: building select SQL (`plan_select`, `load_by_pk`) is 14% of it, and the commit batch building its statements 4%.

What to try next, by expected payoff:
1. **SQL text from `sql_cache`** for the cache's selects (`plan_select`, `load_by_pk`, `load_by_pks`) and the commit batch's statements, as the find and write paths have it.
2. **Kept rows shared, not copied.** Keep rows as `Arc<FieldMap>` and give the `EntityValue` the same `Arc` (it holds its fields in one), copying only a row a held write changes; the kept list the same.
3. **No string key for a list find**: look the list up through `with_lookup_key`'s thread buffer, and build the owned key only to keep one.
4. **No `HashMap` for the key equalities**: the key fields are few; a small vector.
5. A CPU profile (samply) of the S rows with the cache on, to see what is not allocation: the overlay's condition checks and sorting, `Value::clone` (11% of S4's allocations, much of it the copies above).
6. Then the read paths of the five services themselves: S5 reads 8 rows from the database a call with the cache on, S3 8, S4 15; rows passed down rather than read again by key, as the plan's table lists, now cost nothing when kept, so what is left is reads of rows read once.

## Round 15: the transaction cache, tuned

Round 14's first three items, then a CPU profile of S3, S4 and S5 with the cache on and off.

**How the CPU profile was taken.** samply could not map its buffers here (one per thread, against `perf_event_mlock_kb` 516 and `ulimit -l` 8 MB, with the test's many threads), and `perf record --call-graph dwarf` lost chunks and with them the binary's mappings. What worked: the speed test built once with frame pointers in its own target directory (`RUSTFLAGS="-C force-frame-pointers=yes" CARGO_TARGET_DIR=target/fp cargo test --release -p moqui-host --test tx_cache_speed --no-run`, 3 minutes), then `perf record -F 999 -g -m 64` on one row (`TX_CACHE_SPEED_ROW`, 2,000 calls), and `perf script` folded to inclusive and self counts of the samples under the timed row. Both need `perf_event_paranoid` at 1.

What changed:
1. **SQL text cached** (Round 14 item 1). The cache's selects take the entity's read shape (the `SELECT ... FROM` head and the columns, rendered once) and render only the `WHERE`; a find by key and each commit batch statement take their whole SQL from `sql_cache` by entity, dialect, operation and columns, the key values typed as the condition would bind them (`moqui_db::sql::typed_value`).
2. **Kept rows shared** (item 2). Kept rows, kept lists and a find's rows in memory are `Arc<FieldMap>`, given out as the `EntityValue`'s own fields; a held write copies a row only to change it.
3. **No key built per find** (items 3, 4): a kept list is looked up through the thread's key buffer, and the key equalities are a small vector.
4. **A regular expression is compiled once a thread.** S5's profile, on and off alike, had a quarter of its samples in `authz::is_permitted` → `pattern_matches` → `CompiledRegex::new`: every `matches` comparison (each authorization and tarpit check of a name pattern, on every artifact push) parsed its pattern again. `regex_match::is_full_match` keeps each thread's compiled patterns (up to 512). S5 went from 5,449 to 3,994 allocations a call with the cache on, 5,384 to 3,933 off, and every call that checks a pattern gains.
5. **A secondary sequence reads one field.** `set_sequenced_id_secondary` read every sibling row whole to find the highest number: 20% of S4 with the cache on, more with each version a page gets. It selects the sequence field only (in a hold frame the overlay still sees held rows).
6. **The transaction span's attributes** are made only when the span records.

With the cache on, S3's database reads are the same as off (it reads each row once), and its plans and row mapping cost the same; what the cache adds is about 3% of overlay bookkeeping and the span's attributes, and what it saves is two waited round trips. In CPU samples S3 is now level (1,815 on, 1,866 off at 2,000 calls).

Embedded Postgres:

```
  operation                                        calls  median ms   us/call    calls/s    allocs      bytes      kept    sql     rt        min-max ms
  S1 heartbeat#UserStatus, cache off                 200     57.982    289.91       3449    1351.2     141510     10988   6.29   7.29   52.579-63.958  
  S3 send#Message, cache off                         200    213.950   1069.75        935    4274.9     465367     25142  14.00  15.01  201.455-227.085 
  S2 mark#Read, cache off                            200     34.272    171.36       5836     983.9     111198      8288   3.00   4.00   33.526-43.316  
  S4 update#WikiPage, cache off                      200    711.292   3556.46        281   20558.9    3724504    103941 116.00 117.00  612.237-871.294 
  S5 set#MessageFlags, cache off                     200    198.166    990.83       1009    3850.0     486377     22378  15.99  21.00  165.568-339.848 
  F4 audited update UserAccount.username, cache off     200     19.953     99.76      10024     444.9      63068      1821   3.04   4.06   19.010-26.821  
  F5 fed update EmailMessage.isFlagged, cache off     200    167.998    839.99       1190    1457.2     241933      2879   8.93  13.90   70.502-310.366 
  S1 heartbeat#UserStatus, cache on                  200     53.483    267.42       3739    1269.5     127836      7990   3.03   4.30   50.375-59.789  
  S3 send#Message, cache on                          200    205.992   1029.96        971    4393.2     482008     21639  10.00  12.00  199.839-234.706 
  S2 mark#Read, cache on                             200     36.692    183.46       5451    1031.2     113893      8289   3.00   4.00   34.925-44.274  
  S4 update#WikiPage, cache on                       200    492.098   2460.49        406   18275.3    3843733     20932  16.00  18.00  361.083-625.851 
  S5 set#MessageFlags, cache on                      200    224.565   1122.83        891    4022.3     490835     17356  12.99  18.98  161.126-375.154 
  F1 notification persist, 3 users, cache on         200     26.164    130.82       7644     760.9      85554       -53   2.03   5.06   25.826-28.861  
  F4 audited update UserAccount.username, cache on     200     17.635     88.18      11341     520.4      74580       930   1.02   3.04   17.185-21.989  
  F5 fed update EmailMessage.isFlagged, cache on     200    204.471   1022.36        978    1486.2     238147      2879   6.99  12.98   80.109-346.805 
  F3 session start and password login, cache on      200   2246.071  11230.35         89     721.7   20006430      3675   3.04   7.06 2230.358-2252.488
  transaction cache per call                      frames    held   mem rd    db rd    early batch st batch rt
  F1 notification persist, 3 users, cache on        2.00    4.00     0.00     0.00     0.00     4.00     1.00
  F3 session start and password login, cache on     1.00    3.00     0.00     0.00     0.00     2.00     1.00
  F4 audited update UserAccount.username, cache on    1.00    2.00     0.00     1.00     0.00     2.00     1.00
  F5 fed update EmailMessage.isFlagged, cache on    1.00    1.00     1.00     2.00     0.00     1.00     1.00
  S1 heartbeat#UserStatus, cache on                 1.00    0.26     3.00     3.00     0.00     0.26     0.26
  S2 mark#Read, cache on                            1.00    0.00     0.00     3.00     0.00     0.00     0.00
  S3 send#Message, cache on                         1.00    3.00     1.00     8.00     0.00     3.00     1.01
  S4 update#WikiPage, cache on                      1.00    4.00    96.00    16.00     0.00     4.00     1.00
  S5 set#MessageFlags, cache on                     1.00    1.00     2.00     8.00     0.00     1.00     1.00
```

With 200 us of added delay:

```
  operation                                        calls  median ms   us/call    calls/s    allocs      bytes      kept    sql     rt        min-max ms
  S1 heartbeat#UserStatus, cache off                 200    441.847   2209.23        453    1414.3     147302     11484   7.00   8.00  440.024-464.197 
  S3 send#Message, cache off                         200    949.779   4748.90        211    4277.4     465389     25062  14.00  15.01  944.728-987.326 
  S2 mark#Read, cache off                            200    264.810   1324.05        755     984.1     111161      8288   3.00   4.00  257.419-335.715 
  S4 update#WikiPage, cache off                      200   6183.164  30915.82         32   20558.8    3724531    103941 116.00 117.00 5976.054-6379.439
  S5 set#MessageFlags, cache off                     200    906.858   4534.29        221    3845.5     485834     22378  15.98  20.98  890.143-958.830 
  F4 audited update UserAccount.username, cache off     200    224.391   1121.96        891     444.6      63012      1816   3.04   4.07  221.843-238.854 
  F5 fed update EmailMessage.isFlagged, cache off     200    470.379   2351.90        425    1458.7     241870      2879   8.98  13.97  452.524-492.509 
  S1 heartbeat#UserStatus, cache on                  200    306.276   1531.38        653    1371.0     139732      7990   3.03   5.03  303.782-339.654 
  S3 send#Message, cache on                          200    777.056   3885.28        257    4395.0     481970     21639  10.00  12.00  750.066-855.896 
  S2 mark#Read, cache on                             200    267.565   1337.82        747    1032.0     113948      8295   3.00   4.00  260.916-277.726 
  S4 update#WikiPage, cache on                       200   1543.798   7718.99        130   18275.7    3843675     20932  16.00  18.00 1382.263-6410.464
  S5 set#MessageFlags, cache on                      200    804.719   4023.60        249    4016.4     490174     17356  12.98  18.98  788.156-869.142 
  F1 notification persist, 3 users, cache on         200    278.549   1392.75        718     761.3      85522       -45   2.04   5.08  270.893-334.925 
  F4 audited update UserAccount.username, cache on     200    177.955    889.77       1124     520.5      74549       929   1.02   3.04  173.906-194.626 
  F5 fed update EmailMessage.isFlagged, cache on     200    429.172   2145.86        466    1488.1     238115      2879   6.97  12.97  415.883-476.273 
  F3 session start and password login, cache on      200   2575.081  12875.41         78     721.7   20006232      3521   3.04   7.06 2559.477-2587.922
  transaction cache per call                      frames    held   mem rd    db rd    early batch st batch rt
  F1 notification persist, 3 users, cache on        2.00    4.00     0.00     0.00     0.00     4.00     1.00
  F3 session start and password login, cache on     1.00    3.00     0.00     0.00     0.00     2.00     1.00
  F4 audited update UserAccount.username, cache on    1.00    2.00     0.00     1.00     0.00     2.00     1.00
  F5 fed update EmailMessage.isFlagged, cache on    1.00    1.00     1.00     2.00     0.00     1.00     1.00
  S1 heartbeat#UserStatus, cache on                 1.00    1.00     3.00     3.00     0.00     1.00     1.00
  S2 mark#Read, cache on                            1.00    0.00     0.00     3.00     0.00     0.00     0.00
  S3 send#Message, cache on                         1.00    3.00     1.00     8.00     0.00     3.00     1.00
  S4 update#WikiPage, cache on                      1.00    4.00    96.00    16.00     0.00     4.00     1.00
  S5 set#MessageFlags, cache on                     1.00    1.00     2.00     8.00     0.00     1.00     1.00
```

| Call, us | off, local | on, local | off, delay | on, delay |
|---|---|---|---|---|
| S1 `heartbeat#UserStatus` | 290 | 267 | 2,209 | 1,531 |
| S2 `mark#Read` | 171 | 183 | 1,324 | 1,338 |
| S3 `send#Message` | 1,070 | 1,030 | 4,749 | 3,885 |
| S4 `update#WikiPage` | 3,556 | 2,460 | 30,916 | 7,719 |
| S5 `set#MessageFlags` | 991 | 1,123 | 4,534 | 4,024 |
| F4 audited update | 100 | 88 | 1,122 | 890 |
| F5 fed update | 840 | 1,022 | 2,352 | 2,146 |

Over a local socket the cache is now as fast or faster for S1, S3, S4 and F4. S5 and F5 medians still show it slower at 200 calls, with min-max ranges three times as wide as the difference (the feed's pending rows are processed by job workers beside them); at 2,000 calls with perf, S5 was 1,223 us on and 1,203 off, within noise. Census 4 (every frame holding) passes 391 of 393 framework and example features, and strict mode with the five services marked passes all 393.

Next:
1. S5 and F5 with the feed workers off (`scheduled_job_check_time` is already 0; the pending rows are drained by `registerOnCommit`-style async calls) so their rows time one thing.
2. The data feed at commit (F5): the per-touch re-read and walk to the primary entity, and the definition probe.
3. The wiki service's own reads: S4 still reads 16 rows from the database a save, and its resource reads are a third of it.
4. Allocation: `Value::clone` is still the most frequent allocation site, much of it maps built to call services.

## Round 16: quiet rows, a wiki save's reads, round trips at commit, copies

Round 15's four next steps, against the same `tx_cache_speed` rows, embedded Postgres and the docker Postgres with 200 µs of delay.

1. **Quiet rows.** The test boots its factory with the inline executor (`Executor::Inline`) and runs the async work a row queued (the data feed's receive of each pending row, a chat message's indexing) after the row, outside its time and its counts. S5 and F5 were the rows that showed the cache slower: their min-max ranges were three times the difference, from feed receives running on the workers beside them. Quiet, they are level or faster with the cache on (S5 692 off, 670 on; F5 221 off, 211 on), with ranges of a few percent. `rt` was the process's waited round trips, so it counted the workers' too: S5's went from 21 to 13 off.
2. **The data feed at commit (F5).** A fed update of a primary entity rereads nothing (the per-touch reread is for related entities only). What it waited on was the savepoint around the pending rows: `SAVEPOINT`, the rows, `RELEASE`, each a round trip. A savepoint with nothing unanswered before it is now sent with the next request, a release always (it fails only in a transaction already failed), and a `COMMIT` goes out behind the writes still unanswered when the server checks them all (a failed write ends the transaction as a rollback on the server, and its failure is the error); writes with a check made here (an update that must change a row) are still waited for first. F5 went from 7 waited round trips to 4 on, 8 to 5 off. The delivery of pending rows (`realtime_feeds`) reads `DataFeed` and `DataFeedDocument` through the entity cache. Every message write also ran `note#MessageChange`'s count of awake `EmailUserState` rows (one waited round trip, S5 and F5 alike); the count is cached now (`EmailUserState` is `cache="false"`, not `never`, and a write of one clears it).
3. **The wiki save's reads (S4).** `ec.resource.getLocationReference` read a file's size and last-modified time when the reference was made, and a directory's children each with its size: the space directory's reference listed every page of the space and read each one's `DbResourceFile` row, bytes included. A reference now reads size, last-modified time and children when a script asks for them (`resourceStat`, `resourceEntries`), `getDirectoryEntries` builds its entries from the listing (no lookup of each child), and a `dbresource` reference lists a directory's children the first time they are wanted. S4 reads 14 rows from the database (16 before), 59 from the frame (96), and a space with many pages no longer costs a read of each on every save.
4. **Copies of service values.** A service's out-parameters are moved out of its context frame, which is popped next (`ContextStack::take`, when nothing else shares the frame), not copied; parameter validation takes each value out of the parameter map to clean it and puts it back, rather than copying it; a script's bare names are written back without a second copy (`put_nearest`); a notification's message is copied once, into the wrapped message. S1 went from 1,267 to 1,150 allocations a call on, S3 from 4,393 to 4,124, S5 from 3,980 to 3,820.

Embedded Postgres:

```
  operation                                        calls  median ms   us/call    calls/s    allocs      bytes      kept    sql     rt        min-max ms
  S1 heartbeat#UserStatus, cache off                 200     62.392    311.96       3206    1238.9     131194     11002   6.32   7.32   55.245-74.248  
  S3 send#Message, cache off                         200    193.382    966.91       1034    4005.2     452919     36499  13.00  14.00  180.802-235.664 
  S2 mark#Read, cache off                            200     33.156    165.78       6032     928.9     105765      8288   3.00   4.00   32.771-33.791  
  S4 update#WikiPage, cache off                      200    603.519   3017.59        331   17143.2    3376802     71474  77.00  78.00  421.925-737.004 
  S5 set#MessageFlags, cache off                     200    132.262    661.31       1512    3649.2     465143     21728  10.00  12.00  129.417-153.307 
  F4 audited update UserAccount.username, cache off     200     19.208     96.04      10412     444.8      63075      1816   3.02   4.04   18.797-20.215  
  F5 fed update EmailMessage.isFlagged, cache off     200     42.145    210.72       4746    1365.0     232250      2222   3.00   5.00   40.959-42.888  
  S1 heartbeat#UserStatus, cache on                  200     43.738    218.69       4573    1150.3     116789      7990   3.00   4.22   43.162-44.387  
  S3 send#Message, cache on                          200    185.932    929.66       1076    4123.6     457795     21639   9.00  11.00  184.516-205.652 
  S2 mark#Read, cache on                             200     35.287    176.43       5668     976.2     108488      8289   3.00   4.00   34.170-38.703  
  S4 update#WikiPage, cache on                       200    457.392   2286.96        437   16313.2    3689458     19205  14.00  16.00  373.611-593.342 
  S5 set#MessageFlags, cache on                      200    130.163    650.82       1537    3820.1     469523     16699   7.00  10.00  127.870-138.054 
  F1 notification persist, 3 users, cache on         200     25.347    126.73       7891     760.9      85554       -53   2.02   5.04   25.076-26.101  
  F4 audited update UserAccount.username, cache on     200     18.432     92.16      10851     520.5      74587       929   1.02   3.04   17.429-18.654  
  F5 fed update EmailMessage.isFlagged, cache on     200     40.884    204.42       4892    1393.9     228446      2222   1.00   4.00   38.860-56.854  
  F3 session start and password login, cache on      200   2222.704  11113.52         90     721.5   20006349      3615   3.00   7.01 2214.628-2237.784
```

With 200 us of added delay:

```
  operation                                        calls  median ms   us/call    calls/s    allocs      bytes      kept    sql     rt        min-max ms
  S1 heartbeat#UserStatus, cache off                 200    453.784   2268.92        441    1299.6     136820     11484   7.00   8.00  451.638-494.596 
  S3 send#Message, cache off                         200    927.556   4637.78        216    4007.8     441175     25062  13.00  14.00  909.263-968.772 
  S2 mark#Read, cache off                            200    263.871   1319.36        758     929.2     105757      8288   3.00   4.00  259.214-280.670 
  S4 update#WikiPage, cache off                      200   4365.656  21828.28         46   17143.0    3376548     71480  77.00  78.00 4158.628-4518.217
  S5 set#MessageFlags, cache off                     200    829.678   4148.39        241    3647.6     465206     21721  10.00  12.00  823.107-848.031 
  F4 audited update UserAccount.username, cache off     200    224.958   1124.79        889     444.7      63013      1816   3.02   4.04  222.314-288.948 
  F5 fed update EmailMessage.isFlagged, cache off     200    311.064   1555.32        643    1367.0     232224      2222   3.00   5.00  292.732-338.102 
  S1 heartbeat#UserStatus, cache on                  200    307.537   1537.69        650    1256.8     129270      7990   3.00   5.00  304.408-365.897 
  S3 send#Message, cache on                          200    779.832   3899.16        256    4125.9     458299     21907   9.00  11.00  728.941-834.142 
  S2 mark#Read, cache on                             200    265.796   1328.98        752     976.1     108410      8289   3.00   4.00  265.315-269.914 
  S4 update#WikiPage, cache on                       200   1450.829   7254.15        138   22712.6    5660751     19205  14.00  16.00 1281.823-1468.884
  S5 set#MessageFlags, cache on                      200    703.910   3519.55        284    3818.2     469423     16699   7.00  10.00  684.362-721.904 
  F1 notification persist, 3 users, cache on         200    292.491   1462.45        684     760.9      85466       -53   2.02   5.04  276.229-374.611 
  F4 audited update UserAccount.username, cache on     200    188.145    940.73       1063     520.5      74550       929   1.02   3.04  179.964-226.337 
  F5 fed update EmailMessage.isFlagged, cache on     200    261.473   1307.36        765    1395.6     228373      2222   1.00   4.00  252.727-306.980 
  F3 session start and password login, cache on      200   2575.368  12876.84         78     723.6   20006551      3629   3.02   7.03 2544.403-2604.865
```

| Call, us | off, local | on, local | off, delay | on, delay | Round 15 off / on, delay |
|---|---|---|---|---|---|
| S1 `heartbeat#UserStatus` | 312 | 219 | 2,269 | 1,538 | 2,209 / 1,531 |
| S2 `mark#Read` | 166 | 176 | 1,319 | 1,329 | 1,324 / 1,338 |
| S3 `send#Message` | 967 | 930 | 4,638 | 3,899 | 4,749 / 3,885 |
| S4 `update#WikiPage` | 3,018 | 2,287 | 21,828 | 7,254 | 30,916 / 7,719 |
| S5 `set#MessageFlags` | 661 | 651 | 4,148 | 3,520 | 4,534 / 4,024 |
| F4 audited update | 96 | 92 | 1,125 | 941 | 1,122 / 890 |
| F5 fed update | 211 | 204 | 1,555 | 1,307 | 2,352 / 2,146 |

With quiet rows every call but `mark#Read` (which holds nothing) is as fast or faster with the cache on, over a local socket and with the delay. S4's allocations depend on how many versions the page has by then (each save diffs against the last), so they are not comparable between runs of a different length. How these were found: the server's statement log (`log_statement = all` on the docker Postgres, two calls of a row) for the reads and round trips, heaptrack's stacks (`heaptrack_print -F`, demangled, folded under the timed closure) for the copies.

Next:
1. Script host bridging is now the largest allocation site of S3: `HostGlobalNames::name_of` and the entity bindings turn a property atom into a `String` on each bare-name or field read (about 9% of S3's allocations). An atom-keyed memo of names would take most of it.
2. Parameter maps a Rust service gets are borrowed, so a service that keeps one (`push#Signal`'s message) copies it; services taking their parameters by value would save that copy and the one `validate::clean` makes of the whole map.
3. A held update carries the "must change a row" check, so its commit batch waits before the `COMMIT`. Where the frame read the row under a lock (a `for-update` find) the check could be dropped and the `COMMIT` sent with the batch.
4. The wiki save still reads the whole version list of its file for the next version name, and the past version's row by key after it; the second is answered by the first.

## Round 17: script names, service parameters, commits after locks

The framework layers Round 16 found under the timed services, which every service and transaction goes through.

1. **Property names read in place.** A bare-name read in a script (`HostGlobalNames`) and a field read of an entity value or list (`atom_name`) turned the property's atom into a Rust `String` each time. They read QuickJS's own text now (`AtomText`, a `JS_ToCStringLen` view: an ASCII name is the string's bytes, no copy).
2. **Service parameters handed over.** A service called from a script (`ec.service.sync()...call()`, which every `<service-call>` is) gives its parameter map to the call (`sync_with_owned`), and validation cleans it in place: `validate::clean` copied the whole map first. A builder's parameters go with its call; nothing in the framework or the components calls one builder twice.
3. **A commit after a lock does not wait for its batch.** The rows a locking find of a transaction-cache frame read are noted, and a held update of one goes out without the "must change a row" check (no other transaction can delete it before the commit); a locking find goes through the frame even before it holds anything of the entity, so it keeps the rows. The frame's batch is no longer waited for at its end: the `COMMIT` goes out behind it, one round trip for both, when no write in it is checked here. The data feed's insert, which runs between them, waits for the batch first when it has a statement of its own to send, and passes a failure of it on (it would otherwise be logged as the feed's and committed past); a `COMMIT` the server answers with `ROLLBACK` is an error. S1 and F1 went from 5 waited round trips to 4.
4. **The wiki save's history row** (asked for while in there): the version list read for the next version name says whether the replaced version has its history row, which was then read again by key. S4 reads 13 rows (14).

Allocations a call, cache on: S1 1,150 to 1,095, S2 976 to 872, S3 4,124 to 3,784, S5 3,820 to 3,599 (6 to 11% fewer). The `batch rt` column is 0 now: the batch's wait is the commit's.

Embedded Postgres:

```
  operation                                        calls  median ms   us/call    calls/s    allocs      bytes      kept    sql     rt        min-max ms
  S1 heartbeat#UserStatus, cache off                 200     55.292    276.46       3617    1173.3     127092     10977   6.28   7.28   49.809-74.299  
  S3 send#Message, cache off                         200    192.427    962.14       1039    3651.1     423263     25057  13.00  14.00  184.511-217.714 
  S2 mark#Read, cache off                            200     34.188    170.94       5850     825.2     102616      8294   3.00   4.00   32.838-45.222  
  S4 update#WikiPage, cache off                      200    637.958   3189.79        314   18639.2    3407559     70479  76.00  77.00  437.901-800.042 
  S5 set#MessageFlags, cache off                     200    135.784    678.92       1473    3426.1     452029     21721  10.00  12.00  132.643-141.995 
  F4 audited update UserAccount.username, cache off     200     19.777     98.89      10113     447.8      63123      1826   3.02   4.04   19.120-27.024  
  F5 fed update EmailMessage.isFlagged, cache off     200     43.914    219.57       4554    1265.0     223769      2222   3.00   5.00   42.741-55.064  
  S1 heartbeat#UserStatus, cache on                  200     43.768    218.84       4570    1094.7     113587     10175   3.00   4.00   43.529-45.504  
  S3 send#Message, cache on                          200    185.504    927.52       1078    3784.1     442673     21700   9.00  11.01  184.282-188.746 
  S2 mark#Read, cache on                             200     34.474    172.37       5801     872.0     104508     20404   3.00   4.00   34.363-35.312  
  S4 update#WikiPage, cache on                       200    470.705   2353.53        425   18221.6    3732789     18210  13.00  15.00  315.263-601.759 
  S5 set#MessageFlags, cache on                      200    138.066    690.33       1449    3599.2     456447     16699   7.00  10.00  133.127-153.781 
  F1 notification persist, 3 users, cache on         200     25.007    125.04       7998     764.9      85649       -53   2.02   4.04   23.563-25.977  
  F4 audited update UserAccount.username, cache on     200     18.462     92.31      10833     525.3      74673       929   1.02   3.04   17.739-23.841  
  F5 fed update EmailMessage.isFlagged, cache on     200     44.338    221.69       4511    1296.0     220029      2222   1.00   4.00   40.005-53.393  
  F3 session start and password login, cache on      200   2238.679  11193.39         89     725.7   20006199      3393   3.01   6.02 2221.060-2246.282
```

With 200 us of added delay:

```
  operation                                        calls  median ms   us/call    calls/s    allocs      bytes      kept    sql     rt        min-max ms
  S1 heartbeat#UserStatus, cache off                 200    434.360   2171.80        460    1237.9     133007     11484   7.00   8.00  427.113-851.949 
  S3 send#Message, cache off                         200    906.531   4532.65        221    3650.9     423131     25062  13.00  14.00  882.448-954.706 
  S2 mark#Read, cache off                            200    254.573   1272.86        786     824.9     102564      8288   3.00   4.00  251.408-280.231 
  S4 update#WikiPage, cache off                      200   4210.200  21051.00         48   18638.5    3407267     70479  76.00  77.00 4010.921-4484.446
  S5 set#MessageFlags, cache off                     200    776.075   3880.38        258    3431.5     452066     21721  10.00  12.00  758.849-810.187 
  F4 audited update UserAccount.username, cache off     200    221.872   1109.36        901     447.7      63072      1816   3.02   4.04  218.709-245.209 
  F5 fed update EmailMessage.isFlagged, cache off     200    293.355   1466.77        682    1264.5     223660      2222   3.00   5.00  285.598-317.858 
  S1 heartbeat#UserStatus, cache on                  200    272.710   1363.55        733    1200.9     125891      7990   3.00   4.00  255.334-293.784 
  S3 send#Message, cache on                          200    733.936   3669.68        273    3784.8     442626     21645   9.00  11.00  731.766-798.330 
  S2 mark#Read, cache on                             200    258.937   1294.68        772     872.6     104512     20404   3.00   4.00  257.524-274.781 
  S4 update#WikiPage, cache on                       200   1344.001   6720.00        149   22221.8    4744050     18210  13.00  15.00 1208.382-1414.063
  S5 set#MessageFlags, cache on                      200    675.257   3376.29        296    3604.5     456416     16699   7.00  10.00  652.626-862.853 
  F1 notification persist, 3 users, cache on         200    228.507   1142.54        875     764.9      85561       -53   2.02   4.04  223.176-237.198 
  F4 audited update UserAccount.username, cache on     200    178.367    891.84       1121     525.6      74653       929   1.02   3.04  172.939-189.127 
  F5 fed update EmailMessage.isFlagged, cache on     200    250.352   1251.76        799    1295.3     219896      2222   1.00   4.00  241.763-271.702 
  F3 session start and password login, cache on      200   2516.209  12581.05         79     727.7   20006726      3755   3.02   6.03 2495.295-2760.023
```

| Call, us | off, local | on, local | off, delay | on, delay | Round 16 on, delay |
|---|---|---|---|---|---|
| S1 `heartbeat#UserStatus` | 276 | 219 | 2,172 | 1,364 | 1,538 |
| S2 `mark#Read` | 171 | 172 | 1,273 | 1,295 | 1,329 |
| S3 `send#Message` | 962 | 928 | 4,533 | 3,670 | 3,899 |
| S4 `update#WikiPage` | 3,190 | 2,354 | 21,051 | 6,720 | 7,254 |
| S5 `set#MessageFlags` | 679 | 690 | 3,880 | 3,376 | 3,520 |
| F1 notification persist | | 125 | | 1,143 | 1,462 |
| F4 audited update | 99 | 92 | 1,109 | 892 | 941 |
| F5 fed update | 220 | 222 | 1,467 | 1,252 | 1,307 |

Next, all in shared layers:
1. A find by an `in` of primary keys the frame has all kept (S1 reads the `UserStatus` row it just locked, by `userId in [..]`) still goes to the database; the frame could answer it as it answers a find by one key.
2. A held update of a row the frame only read keeps its check, so its commit waits for the batch (S3, S5). Most services lock what they change; where a service reads and updates without a lock the check is what tells a lost row, so this stays unless the update is made conditional on the server (a statement that fails on no row).
3. Script values still copy across the bridge: a JS object's keys become `String`s and its string values host strings (`ObjectIter`, `String::to_string`), about 7% of S3's allocations.
4. A Rust service gets its parameters borrowed, so one that keeps the map copies it.
5. A deferred statement on a cold connection still waits to prepare (Round 5's item 3).

## Round 18: finds by keys in memory, the update check on the server, cold statements

Round 17's next steps.

1. **A find by an `in` of keys the frame knows** (rows it read or created, a one-field primary key, no page, no order of more than one row) is answered from memory, the held writes on the rows, in the order of the keys; one key the frame does not know sends it to the database. S1 read the `UserStatus` row it had just locked by `userId in [..]`: it reads 2 rows from the database now, and waits 3 round trips (4).
2. **The "must change a row" check on the server.** On Postgres a held update the frame did not lock goes out as `WITH moqui_changed AS (UPDATE ... RETURNING 1) SELECT 1 / COUNT(*) FROM moqui_changed`: no row changed is a `division_by_zero` (SQLSTATE 22012) on the server, which fails the transaction, and the driver reports it as the update's "record was not found". Nothing here waits on the check, so the `COMMIT` goes out behind the batch (`DbConnection::execute_deferred_update`; other drivers check the count here, as before). A commit with a data-feed insert still waits once for the batch before the feed's savepoint (Round 17), so S3 and S5, whose entities are fed, keep their count; a frame that writes no fed entity saves the round trip.
3. **Script objects to host maps** are made at their size (QuickJS knows the count of own properties first) instead of grown, which reallocated the entries and rehashed. The keys and string values are still one copy each: host strings are owned.
4. **A Rust service's parameters** stay borrowed: the call keeps its cleaned map after the service runs (post-service rules, tx-commit rules, callbacks), so handing it over would only move the copy. Not done.
5. **A cold deferred statement** (the first of its SQL on a connection, in a transaction, no parameter bytes) is prepared as it is sent: Parse, Bind and Execute together, parameters in text for the server to type, no wait (`send_parse_execute`). It is kept in the cache as not described; a waited call or one with bytes prepares it again with its types. A rollback, or a rollback to a savepoint, drops the statements prepared so in the transaction, as one sent behind a failed statement was never prepared.

Embedded Postgres:

```
  operation                                        calls  median ms   us/call    calls/s    allocs      bytes      kept    sql     rt        min-max ms
  S1 heartbeat#UserStatus, cache off                 200     65.916    329.58       3034    1162.7     122003     11016   6.33   7.33   49.813-73.676  
  S3 send#Message, cache off                         200    200.154   1000.77        999    3605.7     404257     25700  13.00  14.00  187.035-250.229 
  S2 mark#Read, cache off                            200     37.322    186.61       5359     815.5      97843      8288   3.00   4.00   32.608-44.162  
  S4 update#WikiPage, cache off                      200    602.854   3014.27        332   18595.9    3442098     70479  76.00  77.00  443.606-790.346 
  S5 set#MessageFlags, cache off                     200    132.254    661.27       1512    3410.5     438163     21740  10.00  12.00  129.101-149.615 
  F4 audited update UserAccount.username, cache off     200     19.654     98.27      10176     447.6      63110      1816   3.02   4.04   18.563-21.817  
  F5 fed update EmailMessage.isFlagged, cache off     200     43.453    217.27       4603    1244.9     214762      2222   3.00   5.00   40.355-52.782  
  S1 heartbeat#UserStatus, cache on                  200     41.760    208.80       4789    1041.9     103292      9039   2.00   3.00   39.910-46.402  
  S3 send#Message, cache on                          200    187.774    938.87       1065    3747.8     423861     21690   9.00  11.00  184.237-201.220 
  S2 mark#Read, cache on                             200     34.402    172.01       5814     861.9      99693     20404   3.00   4.00   33.961-42.142  
  S4 update#WikiPage, cache on                       200    465.469   2327.34        430   18175.6    3767455     18210  13.00  15.00  339.950-616.590 
  S5 set#MessageFlags, cache on                      200    140.270    701.35       1426    3583.8     442631     16699   7.00  10.00  129.544-157.681 
  F1 notification persist, 3 users, cache on         200     25.182    125.91       7942     764.9      85649       -53   2.02   4.04   23.132-29.434  
  F4 audited update UserAccount.username, cache on     200     17.633     88.17      11342     525.0      74646       925   1.01   3.03   17.136-19.106  
  F5 fed update EmailMessage.isFlagged, cache on     200     39.829    199.15       5021    1276.0     211018      2222   1.00   4.00   38.896-40.175  
  F3 session start and password login, cache on      200   2223.920  11119.60         90     725.7   20006419      3613   3.01   6.02 2214.668-2232.226
```

With 200 us of added delay:

```
  operation                                        calls  median ms   us/call    calls/s    allocs      bytes      kept    sql     rt        min-max ms
  S1 heartbeat#UserStatus, cache off                 200    437.082   2185.41        458    1222.3     127390     11484   7.00   8.00  426.595-456.906 
  S3 send#Message, cache off                         200    899.940   4499.70        222    3606.2     403130     25062  13.00  14.00  886.199-945.163 
  S2 mark#Read, cache off                            200    259.637   1298.19        770     815.5      97827      8288   3.00   4.00  252.924-263.522 
  S4 update#WikiPage, cache off                      200   4230.848  21154.24         47   18592.0    3441963     70479  76.00  77.00 4033.741-4475.850
  S5 set#MessageFlags, cache off                     200    783.436   3917.18        255    3407.4     438226     21721  10.00  12.00  764.074-802.093 
  F4 audited update UserAccount.username, cache off     200    220.752   1103.76        906     447.7      63072      1816   3.02   4.04  219.439-239.481 
  F5 fed update EmailMessage.isFlagged, cache off     200    291.256   1456.28        687    1242.8     214669      2222   3.00   5.00  287.832-319.806 
  S1 heartbeat#UserStatus, cache on                  200    213.976   1069.88        935    1147.9     115733      7005   2.00   3.00  209.896-240.235 
  S3 send#Message, cache on                          200    741.756   3708.78        270    3749.1     423895     21639   9.00  11.00  713.272-821.536 
  S2 mark#Read, cache on                             200    258.041   1290.20        775     862.7      99721     20404   3.00   4.00  257.896-292.689 
  S4 update#WikiPage, cache on                       200   1380.704   6903.52        145   22175.7    4778737     18210  13.00  15.00 1180.876-1391.233
  S5 set#MessageFlags, cache on                      200    683.310   3416.55        293    3579.2     442789     16874   7.00  10.00  653.341-702.313 
  F1 notification persist, 3 users, cache on         200    240.507   1202.53        832     765.0      85812       137   2.02   4.04  232.259-247.759 
  F4 audited update UserAccount.username, cache on     200    176.086    880.43       1136     525.2      74620       925   1.01   3.03  173.378-187.935 
  F5 fed update EmailMessage.isFlagged, cache on     200    248.830   1244.15        804    1273.6     210905      2222   1.00   4.00  243.035-272.930 
  F3 session start and password login, cache on      200   2513.842  12569.21         80     725.7   20006312      3551   3.01   6.02 2509.954-2528.981
```

| Call, us | off, local | on, local | off, delay | on, delay | Round 17 on, delay |
|---|---|---|---|---|---|
| S1 `heartbeat#UserStatus` | 330 | 209 | 2,185 | 1,070 | 1,364 |
| S2 `mark#Read` | 187 | 172 | 1,298 | 1,290 | 1,295 |
| S3 `send#Message` | 1,001 | 939 | 4,500 | 3,709 | 3,670 |
| S4 `update#WikiPage` | 3,014 | 2,327 | 21,154 | 6,904 | 6,720 |
| S5 `set#MessageFlags` | 661 | 701 | 3,917 | 3,417 | 3,376 |
| F1 notification persist | | 126 | | 1,203 | 1,143 |
| F4 audited update | 98 | 88 | 1,104 | 880 | 892 |
| F5 fed update | 217 | 199 | 1,456 | 1,244 | 1,252 |

Next, in shared layers:
1. The data feed's insert at commit waits for the batch before its savepoint, so a fed frame waits twice before its `COMMIT`. The savepoint could go out behind the batch, with a failure of the batch reported as the transaction's (the probe's rollback to a savepoint that was never made would then name it).
2. A data-feed delivery per commit (`process#DataFeedPending`, run on a worker) logs in as the user and reads the pending rows twice; those reads are not in a timed row but are a transaction per fed write.
3. Allocations of a service call itself, now the largest part of S1 and S2 (each about 1,000 a call): the artifact stack frame, telemetry span and authorization checks per call and per entity operation.

## Round 19: the feed behind the batch, a service call's own allocations, the second set of candidates

Round 18's next steps, and the transaction cache applied to framework paths beyond the first set ([TX-Cache, Second
set](TX-Cache.md#second-set-profiling-round-19)).

1. **The feed's savepoint behind the batch.** The data feed's insert at commit no longer waits for the transaction
   cache's batch first unless it reads (the feed configuration not known, a related entity's touch to walk, a write
   without values): the pending rows of a primary-entity write go out in their savepoint behind the batch
   (`DbConnection::savepoint_behind`; on Postgres it waits when a check made here is among the writes), and their one
   wait answers both. A failure of the batch then fails the savepoint's rollback, and is passed on as the transaction's.
2. **An async call's login** (every async service call and job runs as the user who queued it) reads the account
   through the entity cache: one query per call before.
3. **A service call's own allocations.** The regular expression matcher of the authorization and tarpit checks made
   five vectors a match and one more per character of the text; it reuses a thread's buffers (`Scratch`). A bare name a
   script writes again is replaced in place, with no new key. `mark#Read` went from 862 to 779 allocations a call,
   `set#MessageFlags` from 3,584 to 2,993 (before N1's mail cursor, below). The telemetry of each artifact (a metric
   attribute set per push and pop, a span) is next.
4. **Select-field finds kept in a cache frame.** A find of named fields (no lock, aggregate or page) in a frame that
   holds no write of its entity is kept as it was answered, by its key and fields, and answered from memory again,
   until the frame's first held write of the entity. A rule that runs for each row a service writes reads the same
   configuration each time this way (`note#MessageChange`).
5. **The second set**: N1, mail stored with its attachments in one `force-cache` frame (each store path, and the IMAP
   flag sync); N2, `react#Message`, `edit#Message`, `redact#Message`, `vote#Poll` and `mark#AllRead` on the cache; N3,
   `mark#NotificationsViewed` on the cache. The "off" row of N1 turns the cache off on the manager
   (`use-transaction-cache`). Strict mode passes the mail, chat and notification tests and specs.

The speed test's user A keeps a mail change cursor now (N1 needs one), so every write of an EmailMessage makes change
rows: S5 and F5 do more than in earlier rounds (S5 10 statements off, 14 now) and are not comparable with them.

Embedded Postgres:

```
  operation                                        calls  median ms   us/call    calls/s    allocs      bytes      kept    sql     rt        min-max ms
  S1 heartbeat#UserStatus, cache off                 200     56.626    283.13       3532    1076.3     115544     10984   6.29   7.29   48.989-73.511  
  S3 send#Message, cache off                         200    189.735    948.67       1054    3541.8     398265     25056  13.00  14.00  183.698-240.957 
  S2 mark#Read, cache off                            200     34.116    170.58       5862     740.1      92376      8288   3.00   4.00   32.293-43.785  
  S4 update#WikiPage, cache off                      200    647.031   3235.16        309   18137.5    3387975     70479  76.00  77.00  473.583-813.227 
  S5 set#MessageFlags, cache off                     200    171.104    855.52       1169    3704.5     439413     24768  14.02  16.05  167.865-174.743 
  F4 audited update UserAccount.username, cache off     200     19.824     99.12      10089     432.6      56196      1816   3.02   4.04   18.772-24.640  
  F5 fed update EmailMessage.isFlagged, cache off     200     77.352    386.76       2586    1810.2     249751      5266   7.02   9.04   75.878-91.856  
  N1 mail store, 2 attachments, a cursor, cache off     200    455.426   2277.13        439    8706.4     913878     57322  51.20  56.41  405.863-501.892 
  N2 react#Message, cache off                        200    248.471   1242.36        805    4250.2     455381     30297  20.50  21.50  210.395-309.232 
  N3 mark#NotificationsViewed of 5, cache off        200    123.406    617.03       1621    1002.0     105134      7015   6.00   7.00  120.994-125.602 
  S1 heartbeat#UserStatus, cache on                  200     56.168    280.84       3561     969.9      98656      8983   2.00   3.00   47.157-58.634  
  S3 send#Message, cache on                          200    211.982   1059.91        943    3707.5     423712     21586   9.00  11.00  189.233-231.161 
  S2 mark#Read, cache on                             200     34.368    171.84       5819     787.5      94469     20596   3.00   4.00   33.892-49.792  
  S4 update#WikiPage, cache on                       200    553.621   2768.10        361   24425.1    6026371     18210  13.00  15.00  396.585-750.296 
  S5 set#MessageFlags, cache on                      200    167.240    836.20       1196    4010.9     456110     18941  10.02  13.04  164.706-217.416 
  F1 notification persist, 3 users, cache on         200     25.702    128.51       7781     764.9      85785        -7   2.02   4.04   24.146-26.453  
  F4 audited update UserAccount.username, cache on     200     16.930     84.65      11814     510.3      67954       929   1.02   3.04   16.695-18.874  
  F5 fed update EmailMessage.isFlagged, cache on     200     75.619    378.10       2645    1920.9     255442      4461   4.02   7.04   70.183-86.124  
  N1 mail store, 2 attachments, a cursor, cache on     200    424.267   2121.34        471    8220.6     915821     38228  23.07  31.27  406.901-472.928 
  N2 react#Message, cache on                         200    280.447   1402.24        713    4471.0     495139     26438  15.00  16.50  216.241-303.587 
  N3 mark#NotificationsViewed of 5, cache on         200    112.796    563.98       1773    1083.4     120378      3381   1.00   3.00  105.059-129.561 
  F3 session start and password login, cache on      200   2229.547  11147.73         90     723.7   20006416      3456   3.01   6.02 2224.074-2236.685
```

With 200 us of added delay:

```
  operation                                        calls  median ms   us/call    calls/s    allocs      bytes      kept    sql     rt        min-max ms
  S1 heartbeat#UserStatus, cache off                 200    434.243   2171.21        461    1139.1     121356     11484   7.00   8.00  432.027-466.107 
  S3 send#Message, cache off                         200    939.915   4699.57        213    3544.5     398356     25062  13.00  14.00  859.852-984.223 
  S2 mark#Read, cache off                            200    263.328   1316.64        760     740.3      92355      8288   3.00   4.00  251.628-282.395 
  S4 update#WikiPage, cache off                      200   4287.710  21438.55         47   18135.1    3387703     70479  76.00  77.00 4079.085-4580.135
  S5 set#MessageFlags, cache off                     200   1006.943   5034.72        199    3711.8     439369     24775  14.02  16.05  988.543-1017.830
  F4 audited update UserAccount.username, cache off     200    224.149   1120.75        892     432.6      56157      1816   3.02   4.04  221.322-248.278 
  F5 fed update EmailMessage.isFlagged, cache off     200    538.888   2694.44        371    1808.1     249704      5266   7.02   9.04  516.304-565.029 
  N1 mail store, 2 attachments, a cursor, cache off     200   3161.469  15807.34         63    8703.6     913016     57093  51.20  56.40 3090.821-3191.857
  N2 react#Message, cache off                        200   1322.272   6611.36        151    4247.3     455183     30297  20.50  21.50 1294.486-1385.011
  N3 mark#NotificationsViewed of 5, cache off        200    516.428   2582.14        387    1002.4     105121      7015   6.00   7.00  413.481-585.546 
  S1 heartbeat#UserStatus, cache on                  200    250.560   1252.80        798    1064.6     109890      7005   2.00   3.00  237.977-344.037 
  S3 send#Message, cache on                          200    838.971   4194.86        238    3707.4     423582     21586   9.00  11.00  794.509-878.802 
  S2 mark#Read, cache on                             200    271.927   1359.64        735     787.4      94429     20596   3.00   4.00  253.948-369.174 
  S4 update#WikiPage, cache on                       200   1487.281   7436.40        134   30825.1    7866453     18192  13.00  15.00 1194.937-1699.021
  S5 set#MessageFlags, cache on                      200    870.171   4350.86        230    4018.3     456061     18941  10.02  13.04  854.659-961.567 
  F1 notification persist, 3 users, cache on         200    239.315   1196.57        836     764.9      85697        12   2.02   4.04  232.418-338.608 
  F4 audited update UserAccount.username, cache on     200    189.863    949.31       1053     510.5      67933       929   1.02   3.04  179.685-271.560 
  F5 fed update EmailMessage.isFlagged, cache on     200    459.738   2298.69        435    1918.8     255395      4461   4.02   7.04  431.277-526.645 
  N1 mail store, 2 attachments, a cursor, cache on     200   2032.276  10161.38         98    8405.5     927497     38682  24.18  32.38 1975.360-2178.753
  N2 react#Message, cache on                         200   1109.612   5548.06        180    4470.6     495141     26438  15.00  16.50 1093.146-1166.570
  N3 mark#NotificationsViewed of 5, cache on         200    242.905   1214.53        823    1083.4     120334      3381   1.00   3.00  229.221-284.829 
  F3 session start and password login, cache on      200   2566.423  12832.11         78     723.5   20006417      3519   3.00   6.01 2550.873-2585.771
```

| Call, us | off, local | on, local | off, delay | on, delay |
|---|---|---|---|---|
| S1 `heartbeat#UserStatus` | 283 | 281 | 2,171 | 1,253 |
| S3 `send#Message` | 949 | 1,060 | 4,700 | 4,195 |
| S4 `update#WikiPage` | 3,235 | 2,768 | 21,439 | 7,436 |
| N1 mail store, two attachments | 2,277 | 2,121 | 15,807 | 10,161 |
| N2 `react#Message` | 1,242 | 1,402 | 6,611 | 5,548 |
| N3 `mark#NotificationsViewed`, five | 617 | 564 | 2,582 | 1,215 |

Local medians this run are noisy (S3's and N2's ranges are wider than their differences); the delay rows and the
counts are the measure.

Next, in shared layers:
1. DbResource writes: a file in a new directory makes the directory in a frame of its own and looks it up again in
   two more (N1: three short transactions an attachment); the directories made by this process could be known without
   reading them back.
2. The telemetry of each artifact push and pop: a metric attribute set built per call (cached per artifact, keyed by
   the hash `record_artifact` already makes) and a span per entity statement.
3. The agent step bookkeeping (TX-Cache second set, 4) and `send#WebPush` (8), next of the second set.

## Round 20: files in new directories, artifact metrics, agent steps and Web Push

Round 19's next steps.

1. **DbResource writes into a new directory.** A transaction-cache frame read the directory as missing, so after the
   directory was made (in a transaction of its own) every later lookup in the frame still found nothing and went to
   the "made lately" check, a transaction of its own each. Making a directory now drops what the frame read of
   `DbResource` (`cache::forget_frame_reads`), so the next lookup reads it and keeps it; and a file written under a node
   the write itself made does not look for its `DbResourceFile` row. N1 (a message with two attachments): 6 frames to
   4, 31 waited round trips to 28; 9.3 ms with the delay (10.2 in Round 19, 15.6 off).
2. **Artifact metrics.** `moqui.artifact.duration` (and `moqui.artifact.slow`) of each artifact frame took a map of
   attributes built, filtered into the instrument's key-values and dropped each time; the key-values are made once per
   artifact (its type, name, action and sub type) and kept by the thread, and made again when the attribute lists
   change. `mark#Read` went from 787 to 740 allocations a call, `heartbeat#UserStatus` 970 to 912, `send#Message`
   3,708 to 3,564. Spans are the rest of the telemetry cost: each formats its trace and span ids as text and copies its
   name; keeping the ids as bytes in `TraceContext` (text only when exported or propagated) is next.
3. **The agent step bookkeeping** (TX-Cache second set, 4): a model reply's step, calls, message and parts and the run's
   counts, a tool result and a tool call's status, and the run's active time are each one transaction-cache frame
   (`run::in_cache_tx`), and the reply reuses the run row its lease check locked instead of locking it again. A run of
   two model steps and four tool calls (the run tests' in-memory database): 300 waited round trips to 257, 251
   statements to 208; the rest is the run's events, lease and loading.
4. **`send#WebPush`** (second set, 8) held a transaction open across its HTTP calls, one update per subscription after
   each. It is `transaction="ignore"` now, and the outcomes (and the FCM registrations whose key ended) are written
   after the pushes in one transaction-cache frame; when that fails, each on its own, as before.

Embedded Postgres:

```
  operation                                        calls  median ms   us/call    calls/s    allocs      bytes      kept    sql     rt        min-max ms
  S1 heartbeat#UserStatus, cache off                 200     51.055    255.28       3917    1020.4     108990     10963   6.26   7.26   50.002-58.088  
  S3 send#Message, cache off                         200    189.113    945.57       1058    3423.7     384165     25056  13.00  14.00  186.703-216.966 
  S2 mark#Read, cache off                            200     35.421    177.10       5646     701.0      87707      8288   3.00   4.00   33.058-43.576  
  S4 update#WikiPage, cache off                      200    715.526   3577.63        280   18058.5    3378438     70479  76.00  77.00  507.698-853.350 
  S5 set#MessageFlags, cache off                     200    185.144    925.72       1080    3631.3     429981     24768  14.02  16.05  174.680-202.713 
  F4 audited update UserAccount.username, cache off     200     19.675     98.38      10165     419.5      54626      1816   3.02   4.04   18.375-24.759  
  F5 fed update EmailMessage.isFlagged, cache off     200     81.070    405.35       2467    1788.8     248104      5266   7.02   9.04   74.718-108.388 
  N1 mail store, 2 attachments, a cursor, cache off     200    474.794   2373.97        421    8500.0     891535     55230  49.19  54.38  419.788-534.218 
  N2 react#Message, cache off                        200    277.224   1386.12        721    4143.4     442864     30415  20.50  21.50  262.535-299.207 
  N3 mark#NotificationsViewed of 5, cache off        200    131.485    657.43       1521     988.9     103567      7015   6.00   7.00  124.507-146.579 
  S1 heartbeat#UserStatus, cache on                  200     46.380    231.90       4312     911.6      91713      9108   2.00   3.00   39.789-58.805  
  S3 send#Message, cache on                          200    196.452    982.26       1018    3585.9     409339     21586   9.00  11.00  188.940-207.787 
  S2 mark#Read, cache on                             200     38.149    190.75       5243     747.8      89790     20596   3.00   4.00   33.203-45.686  
  S4 update#WikiPage, cache on                       200    601.564   3007.82        332   24346.5    6017024     18210  13.00  15.00  372.628-804.389 
  S5 set#MessageFlags, cache on                      200    173.981    869.90       1150    3938.3     446691     18937  10.02  13.04  172.385-218.955 
  F1 notification persist, 3 users, cache on         200     24.849    124.24       8049     764.9      85783        -7   2.02   4.04   23.947-26.712  
  F4 audited update UserAccount.username, cache on     200     17.626     88.13      11347     497.3      66402       929   1.02   3.04   17.034-23.498  
  F5 fed update EmailMessage.isFlagged, cache on     200     76.720    383.60       2607    1899.8     253814      4461   4.02   7.04   73.520-97.580  
  N1 mail store, 2 attachments, a cursor, cache on     200    480.222   2401.11        416    7930.0     881266     33813  22.13  28.34  387.908-487.532 
  N2 react#Message, cache on                         200    291.332   1456.66        687    4365.9     482695     26438  15.00  16.50  234.486-314.372 
  N3 mark#NotificationsViewed of 5, cache on         200    118.054    590.27       1694    1070.3     118812      3381   1.00   3.00  112.262-124.447 
  F3 session start and password login, cache on      200   2265.197  11325.98         88     723.7   20006415      3456   3.01   6.02 2258.929-2299.150
```

With 200 us of added delay:

```
  operation                                        calls  median ms   us/call    calls/s    allocs      bytes      kept    sql     rt        min-max ms
  S1 heartbeat#UserStatus, cache off                 200    447.570   2237.85        447    1086.7     115149     11484   7.00   8.00  438.131-453.256 
  S3 send#Message, cache off                         200    898.468   4492.34        223    3426.0     384301     25072  13.00  14.00  873.355-977.606 
  S2 mark#Read, cache off                            200    270.052   1350.26        741     701.0      87664      8288   3.00   4.00  258.676-376.664 
  S4 update#WikiPage, cache off                      200   4423.897  22119.48         45   18057.3    3378385     70477  76.00  77.00 4111.431-4620.449
  S5 set#MessageFlags, cache off                     200   1061.853   5309.27        188    3627.6     429921     24768  14.02  16.05 1056.116-4141.572
  F4 audited update UserAccount.username, cache off     200    226.771   1133.85        882     419.4      54582      1816   3.02   4.04  223.105-287.342 
  F5 fed update EmailMessage.isFlagged, cache off     200    541.927   2709.64        369    1795.0     248104      5266   7.02   9.04  511.980-554.127 
  N1 mail store, 2 attachments, a cursor, cache off     200   3117.497  15587.48         64    8501.5     891875     55464  49.20  54.41 3036.982-3159.984
  N2 react#Message, cache off                        200   1378.705   6893.53        145    4143.3     442807     30407  20.50  21.50 1343.914-1406.961
  N3 mark#NotificationsViewed of 5, cache off        200    441.985   2209.92        453     988.9     103524      7015   6.00   7.00  411.643-569.537 
  S1 heartbeat#UserStatus, cache on                  200    243.580   1217.90        821    1013.0     103689      7005   2.00   3.00  235.420-329.740 
  S3 send#Message, cache on                          200    833.667   4168.33        240    3588.5     409418     21586   9.00  11.00  819.428-904.558 
  S2 mark#Read, cache on                             200    272.218   1361.09        735     747.7      89747     20596   3.00   4.00  268.098-332.393 
  S4 update#WikiPage, cache on                       200   1395.689   6978.45        143   24346.7    6016982     18202  13.00  15.00 1264.298-1537.286
  S5 set#MessageFlags, cache on                      200    860.840   4304.20        232    3934.7     447007     19117  10.02  13.04  797.249-900.114 
  F1 notification persist, 3 users, cache on         200    237.817   1189.09        841     764.9      85697        -7   2.02   4.04  235.121-357.426 
  F4 audited update UserAccount.username, cache on     200    191.135    955.67       1046     497.3      66359       929   1.02   3.04  181.936-757.627 
  F5 fed update EmailMessage.isFlagged, cache on     200    443.467   2217.33        451    1905.9     253813      4461   4.02   7.04  422.801-479.628 
  N1 mail store, 2 attachments, a cursor, cache on     200   1858.240   9291.20        108    8106.4     893333     34668  23.20  29.41 1836.298-1887.832
  N2 react#Message, cache on                         200   1150.827   5754.13        174    4366.5     482652     26438  15.00  16.50 1130.941-1172.288
  N3 mark#NotificationsViewed of 5, cache on         200    252.427   1262.14        792    1070.3     118769      3381   1.00   3.00  207.607-271.358 
  F3 session start and password login, cache on      200   2600.000  13000.00         77     725.7   20006858      3756   3.02   6.04 2531.826-2627.838
```

Next, in shared layers:
1. Spans: trace and span ids kept as bytes in `TraceContext`, text only where they are exported or propagated; a
   statement's span (`trace_statement`) and the transaction's span copy names and set attributes per call.
2. The `resource_lock` check of each DbResource write (a waited read per file, its time a parameter so never kept): a
   process-wide note of whether any lock is held would skip it when none is.
3. The rest of the second set: IMAP flag sync preloading its rows (5), `save#Draft` (6), the chat bridge's inbound
   item (7).

## Round 21: span ids as bytes, the lock check, the rest of the second set

Round 20's next steps, on the tree merged with the Commerce and Shop framework work (its moqui-core tests pass).

1. **Span ids as bytes.** `TraceContext` keeps its trace and span ids as their bytes (`HexId<16>`, `HexId<8>`), written as
   hex only where they are exported or propagated: a span no longer formats two ids as text, and a child's parent is
   no longer parsed back from text. Span names are still copied, and the transaction's span sets its attributes per
   call.
2. **The lock check of each DbResource write** read the `ResourceLock` rows around the location from the database (its
   time a parameter, so never kept). A cached count of all lock rows answers "none" from memory, the usual case;
   `ResourceLock` is `cache="false"` (it was `never`), and a write of a lock clears the count here and on cluster
   peers. The count needed a condition every row meets (`lockKey is not null`): the entity cache keeps no find or
   count without a condition, which a first try without one showed (its count went to the database on every write).
3. **The rest of the second set.** The IMAP flag sync reads the rows that change in one `in` find first, so each
   update's rule reads its row as it was from the frame (second set 5); `save#Draft` (6) and the chat bridge's
   `process#InboundItem` (7) are `transaction="cache"`. Strict mode passes the mail, mail client and chat bridge tests
   and specs. A new row, N4, times an autosave of a draft: 15 statements to 9, 5.4 to 4.1 ms with the delay.

Embedded Postgres:

```
  operation                                        calls  median ms   us/call    calls/s    allocs      bytes      kept    sql     rt        min-max ms
  S1 heartbeat#UserStatus, cache off                 200     50.467    252.34       3963     938.9     106712     10960   6.25   7.25   48.334-73.282  
  S3 send#Message, cache off                         200    180.892    904.46       1106    3238.4     379086     25007  13.00  14.00  176.579-206.257 
  S2 mark#Read, cache off                            200     33.382    166.91       5991     652.0      86388      8288   3.00   4.00   32.393-38.344  
  S4 update#WikiPage, cache off                      200    610.623   3053.11        328   17340.2    3351810     70156  76.00  77.00  433.682-792.140 
  S5 set#MessageFlags, cache off                     200    183.408    917.04       1090    3464.3     425344     24720  14.02  16.05  162.267-197.469 
  F4 audited update UserAccount.username, cache off     200     18.771     93.85      10655     407.5      54211      1827   3.02   4.04   18.020-22.392  
  F5 fed update EmailMessage.isFlagged, cache off     200     74.361    371.81       2690    1746.1     246748      5266   7.02   9.04   72.664-87.998  
  N1 mail store, 2 attachments, a cursor, cache off     200    418.671   2093.35        478    7895.1     860738     54530  49.19  54.38  402.223-482.609 
  N2 react#Message, cache off                        200    277.148   1385.74        722    3913.5     436328     30297  20.50  21.50  221.860-286.966 
  N3 mark#NotificationsViewed of 5, cache off        200    118.294    591.47       1691     930.0     101865      7015   6.00   7.00  116.029-145.983 
  N4 save#Draft, an autosave, cache off              200    178.496    892.48       1120    3230.9     434205     24341  15.05  16.93  170.874-217.132 
  S1 heartbeat#UserStatus, cache on                  200     42.498    212.49       4706     853.2      89902      9177   2.00   3.00   37.650-57.610  
  S3 send#Message, cache on                          200    188.883    944.41       1059    3426.2     405324     21744   9.00  11.00  185.146-193.572 
  S2 mark#Read, cache on                             200     33.359    166.79       5995     699.0      88473     20596   3.00   4.00   32.827-36.343  
  S4 update#WikiPage, cache on                       200    539.993   2699.96        370   23994.2    5997778     17887  13.00  15.00  336.184-769.090 
  S5 set#MessageFlags, cache on                      200    164.763    823.82       1214    3796.1     442853     18893  10.02  13.04  163.764-192.353 
  F1 notification persist, 3 users, cache on         200     24.405    122.03       8195     764.9      85656        -7   2.02   4.04   24.276-28.057  
  F4 audited update UserAccount.username, cache on     200     18.511     92.55      10804     491.1      66190       929   1.02   3.04   17.628-24.439  
  F5 fed update EmailMessage.isFlagged, cache on     200     72.905    364.52       2743    1863.1     252746      4461   4.02   7.04   70.527-85.087  
  N1 mail store, 2 attachments, a cursor, cache on     200    411.475   2057.38        486    7435.9     852706     33462  22.00  28.20  340.255-451.386 
  N2 react#Message, cache on                         200    253.081   1265.41        790    4170.0     477306     26438  15.00  16.50  221.230-299.019 
  N3 mark#NotificationsViewed of 5, cache on         200    114.922    574.61       1740    1042.2     118089      3389   1.00   3.00  108.637-130.262 
  N4 save#Draft, an autosave, cache on               200    162.525    812.63       1231    3361.9     414573     15588   9.02  11.66  158.356-179.627 
  F3 session start and password login, cache on      200   2227.195  11135.97         90     724.7   20006508      3612   3.01   6.02 2225.278-2247.279
```

With 200 us of added delay:

```
  operation                                        calls  median ms   us/call    calls/s    allocs      bytes      kept    sql     rt        min-max ms
  S1 heartbeat#UserStatus, cache off                 200    431.243   2156.22        464    1001.5     112771     11484   7.00   8.00  430.213-466.345 
  S3 send#Message, cache off                         200    894.640   4473.20        224    3240.9     379255     25016  13.00  14.00  888.507-960.703 
  S2 mark#Read, cache off                            200    257.706   1288.53        776     652.0      86344      8288   3.00   4.00  251.532-265.382 
  S4 update#WikiPage, cache off                      200   4298.295  21491.48         47   17340.2    3351766     70154  76.00  77.00 3996.679-4413.134
  S5 set#MessageFlags, cache off                     200   1016.452   5082.26        197    3460.3     425256     24720  14.02  16.05  999.274-1078.939
  F4 audited update UserAccount.username, cache off     200    219.631   1098.15        911     407.1      54129      1816   3.02   4.04  217.516-296.219 
  F5 fed update EmailMessage.isFlagged, cache off     200    545.642   2728.21        367    1745.8     246674      5262   7.01   9.03  495.906-564.760 
  N1 mail store, 2 attachments, a cursor, cache off     200   3075.036  15375.18         65    7895.4     860589     54517  49.20  54.40 2964.756-3095.250
  N2 react#Message, cache off                        200   1364.006   6820.03        147    3913.5     436284     30297  20.50  21.50 1317.628-1425.785
  N3 mark#NotificationsViewed of 5, cache off        200    428.105   2140.53        467     930.0     101821      7015   6.00   7.00  414.055-564.493 
  N4 save#Draft, an autosave, cache off              200   1073.802   5369.01        186    3272.7     441516     24524  15.05  17.07 1058.850-1082.222
  S1 heartbeat#UserStatus, cache on                  200    257.616   1288.08        776     958.0     102273      7005   2.00   3.00  235.923-413.403 
  S3 send#Message, cache on                          200    853.603   4268.02        234    3427.9     405157     21538   9.00  11.00  791.563-950.261 
  S2 mark#Read, cache on                             200    280.245   1401.23        714     699.0      88429     20596   3.00   4.00  263.682-325.078 
  S4 update#WikiPage, cache on                       200   1446.512   7232.56        138   30394.2    7837888     17887  13.00  15.00 1204.490-1882.229
  S5 set#MessageFlags, cache on                      200    879.490   4397.45        227    3792.1     442765     18893  10.02  13.04  855.999-1462.538
  F1 notification persist, 3 users, cache on         200    302.733   1513.67        661     764.9      85572        -7   2.02   4.04  228.513-561.836 
  F4 audited update UserAccount.username, cache on     200    196.959    984.80       1015     491.1      66146       929   1.02   3.04  172.645-253.616 
  F5 fed update EmailMessage.isFlagged, cache on     200    461.331   2306.65        434    1863.1     252698      4461   4.02   7.04  425.039-1020.216
  N1 mail store, 2 attachments, a cursor, cache on     200   1836.837   9184.18        109    7623.7     865005     33960  23.20  29.41 1835.809-1882.730
  N2 react#Message, cache on                         200   1107.250   5536.25        181    4170.0     477262     26438  15.00  16.50 1092.013-1124.362
  N3 mark#NotificationsViewed of 5, cache on         200    235.098   1175.49        851    1042.0     118030      3381   1.00   3.00  226.304-252.832 
  N4 save#Draft, an autosave, cache on               200    821.895   4109.47        243    3418.7     424500     15832   9.02  12.04  813.228-838.007 
  F3 session start and password login, cache on      200   2529.531  12647.66         79     724.7   20006433      3581   3.01   6.02 2515.672-2535.874
```

Next:
1. The entity cache's rule that a find or count with no condition is not kept: a count of all rows (the lock count's
   case) or a list of a small configuration entity are worth keeping; `caches` could allow a count with none.
2. The census over mr-boss and mr-mantle specs (TX-Cache "Later rounds"), now that the framework's own paths are on
   the cache: which of their services hold cleanly and which flush early, before turning the cache on for them.
3. `expire#UserStatuses` (second set 9) with a decision on one user's failure failing the batch, and `run#EmailRules`
   (10) merged into the store's frame.

## Round 22: the census over mr-boss and mr-mantle, the frame's reads made right, presence and mail in batches

Round 21's next steps, with the census of the application specs ([TX-Cache, Census 5](TX-Cache.md#census-5-mr-boss-and-mr-mantle)) and the
framework faults it found.

1. **A count with no condition is kept** by the entity cache (`ReadMeta::caches_count`; a list with none still is not):
   any write of the entity drops it, as it matches every row. The lock check counts all lock rows again.
2. **The census of mr-boss and mr-mantle** (`MOQUI_TX_CACHE_EVERYWHERE`): 2,822 of 3,058 features at first; after the
   five framework fixes below, 3,062 of 3,067. Each was a way a frame's read gave another answer than the database:
   - a row created in the frame read back without the fields it did not set; the database gives them, null. A
     fingerprint of the row made in the frame (an optimistic-concurrency revision: standard costs, routings, BOMs) then
     never matched one made later from the database. A held create now carries every field, null where unset.
   - a held row was matched against a condition's text value as text: a time as epoch-millis text never matched a held
     time stamp. The condition is converted at the fields' types first, as SQL binds it (`typed_condition`).
   - a number of another kind (an integer against a decimal) did not compare in memory; it does now, by amount.
   - an encrypted field's condition was not encrypted in the frame's own reads (`plan_select`), so a find by a publish
     token, a contact's encrypted fields, found nothing.
   - a nested frame of its own (a `force-new` call, Mantle's `probe`) that commits a write left the suspended frame on
     the thread reading what it read before; that frame now drops what it read of the entity. And a view read wrote
     the held rows of only its direct members: a member that is a view now stands for the entities under it.
   Left: four features of Contacts, Consent and CustomerServiceAccess, which pass run alone (state from earlier specs),
   and one of WorkCenters.
3. **`expire#UserStatuses`** publishes its users in one batch (`publish#UserStatusBatch`, a `force-cache` frame); a
   batch that fails is split in two and each half tried as a batch, down to the one user that fails, who is logged.
4. **A delivered Inbox message and its rules** (`run#EmailRules`) commit in one frame when no email ECA rule is
   installed; when that fails, the message is stored alone and the rules run after it, as before, so a failing rule
   never loses mail.

Embedded Postgres:

```
  operation                                        calls  median ms   us/call    calls/s    allocs      bytes      kept    sql     rt        min-max ms
  S1 heartbeat#UserStatus, cache off                 200     50.610    253.05       3952     939.4     106753     10963   6.26   7.26   47.964-70.530  
  S3 send#Message, cache off                         200    181.565    907.83       1102    3238.3     379071     25006  13.00  14.00  178.635-223.073 
  S2 mark#Read, cache off                            200     33.693    168.46       5936     652.0      86388      8288   3.00   4.00   31.562-39.070  
  S4 update#WikiPage, cache off                      200    596.968   2984.84        335   17312.2    3349331     69531  75.00  76.00  395.473-768.199 
  S5 set#MessageFlags, cache off                     200    170.960    854.80       1170    3466.3     425354     24720  14.02  16.04  166.528-220.592 
  F4 audited update UserAccount.username, cache off     200     19.247     96.24      10391     407.1      54173      1816   3.02   4.04   18.381-19.583  
  F5 fed update EmailMessage.isFlagged, cache off     200     75.056    375.28       2665    1752.1     246802      5266   7.02   9.04   74.187-81.215  
  N1 mail store, 2 attachments, a cursor, cache off     200    427.489   2137.45        468    7843.1     855858     53280  47.19  52.38  388.416-495.119 
  N2 react#Message, cache off                        200    249.336   1246.68        802    3913.5     436327     30297  20.50  21.50  215.589-288.148 
  N3 mark#NotificationsViewed of 5, cache off        200    115.873    579.37       1726     930.0     101865      7015   6.00   7.00  110.670-140.055 
  N4 save#Draft, an autosave, cache off              200    175.655    878.27       1139    3237.0     435012     24361  15.05  16.95  170.880-225.011 
  S1 heartbeat#UserStatus, cache on                  200     39.313    196.57       5087     868.2      91349      8392   2.00   3.00   38.643-55.745  
  S3 send#Message, cache on                          200    188.865    944.33       1059    3469.8     409243     21538   9.00  11.00  182.533-194.754 
  S2 mark#Read, cache on                             200     36.866    184.33       5425     715.0      90349     20596   3.00   4.00   33.480-41.287  
  S4 update#WikiPage, cache on                       200    577.516   2887.58        346   24144.2    6023751     17262  12.00  14.00  320.260-753.324 
  S5 set#MessageFlags, cache on                      200    188.348    941.74       1062    3828.1     448116     18893  10.02  13.04  163.120-222.801 
  F1 notification persist, 3 users, cache on         200     25.296    126.48       7907     786.9      88304        -7   2.02   4.04   24.652-31.168  
  F4 audited update UserAccount.username, cache on     200     17.743     88.72      11272     496.1      68358       929   1.02   3.04   17.015-20.469  
  F5 fed update EmailMessage.isFlagged, cache on     200     77.271    386.35       2588    1870.1     252832      4461   4.02   7.04   72.744-104.994 
  N1 mail store, 2 attachments, a cursor, cache on     200    389.390   1946.95        514    7482.7     866073     32196  19.98  26.18  328.426-475.964 
  N2 react#Message, cache on                         200    283.468   1417.34        706    4227.0     483858     26438  15.00  16.50  235.080-324.904 
  N3 mark#NotificationsViewed of 5, cache on         200    113.284    566.42       1765    1042.0     118074      3381   1.00   3.00  111.124-125.137 
  N4 save#Draft, an autosave, cache on               200    163.897    819.48       1220    3395.9     419985     15595   9.02  11.67  158.931-198.053 
  F3 session start and password login, cache on      200   2233.529  11167.65         90     751.7   20011418      3706   3.01   6.02 2220.864-2245.009
```

With 200 us of added delay:

```
  operation                                        calls  median ms   us/call    calls/s    allocs      bytes      kept    sql     rt        min-max ms
  S1 heartbeat#UserStatus, cache off                 200    432.818   2164.09        462    1001.9     112801     11499   7.00   8.00  428.037-470.275 
  S3 send#Message, cache off                         200    894.004   4470.02        224    3240.9     379257     25014  13.00  14.00  865.000-980.749 
  S2 mark#Read, cache off                            200    254.009   1270.04        787     652.0      86344      8288   3.00   4.00  252.073-269.975 
  S4 update#WikiPage, cache off                      200   4285.809  21429.05         47   17312.4    3349302     69537  75.00  76.00 3955.844-4453.730
  S5 set#MessageFlags, cache off                     200   1011.065   5055.32        198    3458.3     425258     24720  14.02  16.05  997.171-1016.806
  F4 audited update UserAccount.username, cache off     200    221.020   1105.10        905     407.1      54129      1816   3.02   4.04  217.036-3507.720
  F5 fed update EmailMessage.isFlagged, cache off     200    523.675   2618.38        382    1750.1     246724      5266   7.02   9.04  500.380-549.902 
  N1 mail store, 2 attachments, a cursor, cache off     200   2952.811  14764.05         68    7843.6     856185     53514  47.20  52.41 2906.249-2964.250
  N2 react#Message, cache off                        200   1351.849   6759.25        148    3914.0     436384     30423  20.50  21.50 1314.703-1378.432
  N3 mark#NotificationsViewed of 5, cache off        200    446.024   2230.12        448     930.0     101821      7015   6.00   7.00  417.177-590.535 
  N4 save#Draft, an autosave, cache off              200   1072.941   5364.70        186    3278.7     441579     24531  15.05  17.07 1054.115-1088.860
  S1 heartbeat#UserStatus, cache on                  200    235.721   1178.60        848     975.0     103959      7005   2.00   3.00  211.778-343.208 
  S3 send#Message, cache on                          200    822.481   4112.40        243    3471.9     409355     21538   9.00  11.00  805.108-885.957 
  S2 mark#Read, cache on                             200    260.200   1301.00        769     715.0      90305     20596   3.00   4.00  257.146-318.292 
  S4 update#WikiPage, cache on                       200   1354.605   6773.03        148   24144.2    6023707     17244  12.00  14.00 1195.368-1479.824
  S5 set#MessageFlags, cache on                      200    866.492   4332.46        231    3820.1     448016     18893  10.02  13.04  831.530-919.273 
  F1 notification persist, 3 users, cache on         200    234.555   1172.78        853     786.9      88220        -7   2.02   4.04  227.935-295.305 
  F4 audited update UserAccount.username, cache on     200    176.441    882.20       1134     496.1      68314       929   1.02   3.04  175.975-189.605 
  F5 fed update EmailMessage.isFlagged, cache on     200    424.018   2120.09        472    1868.1     252754      4461   4.02   7.04  417.224-482.269 
  N1 mail store, 2 attachments, a cursor, cache on     200   1732.236   8661.18        115    7673.4     878566     32724  21.20  27.39 1705.967-1786.395
  N2 react#Message, cache on                         200   1111.889   5559.44        180    4227.0     483814     26438  15.00  16.50 1091.474-1135.968
  N3 mark#NotificationsViewed of 5, cache on         200    232.229   1161.14        861    1042.0     118030      3381   1.00   3.00  228.024-256.712 
  N4 save#Draft, an autosave, cache on               200    832.420   4162.10        240    3455.5     429673     15832   9.02  12.04  826.958-847.709 
  F3 session start and password login, cache on      200   2529.334  12646.67         79     753.5   20011353      3566   3.02   6.04 2505.830-2550.111
```

Next:
1. The five census features left, and the spec state they depend on.
2. The cache on by default for kinds of service the census shows hold cleanly (TX-Cache "Later rounds"); the early
   flushes left are Mantle's view reads of rows a service wrote (`refresh#Mode`, `update#ProjectTimeFromTask`) and the
   asset reservations marked `no-tx-cache`.
3. A held time stamp finer than the database keeps (Postgres: microseconds) still reads back finer in its frame.

## Round 23: the cache on for 45 application services, chosen by the census

Round 22's next steps. The census now measures as well as finds: with `MOQUI_TX_CACHE_CENSUS_FILE` set, each
transaction writes a `frame` line when it ends (the service that ended it, its cache mode, its held writes, reads from
memory and from the database, early flushes, and its waited round trips), with the cache everywhere or not. A census
run over every component's specs (108,019 transactions) ranked the services that begin a transaction by what holding
saves them; 45 of them are now `transaction="cache"`: 43 in mr-mantle and mr-boss, and `CallServices.start#Call` and
`VoiceServices.receive#PhoneCall` in the framework. 38 are called only from screens, REST, jobs and specs, never from
another service; the other seven are the order, invoice and production operations (`place#Order`, `approve#Order`,
`add#OrderProductQuantity`, `change#InvoiceState`, `repost#Invoice`, `create#ProductionOrder`,
`transition#ProductionOrder`), whose service callers are bulk, return, billing and demo operations, held under the
census already. Left off for now: services many others call (`create#Payment`, `apply#Settlement`), and those whose
early flushes come from their own view reads (`pack#Shipment`, `ship#OrderPart`, `store#TimeEntry`).

Measured by the specs of mr-mantle, mr-boss and mr-pass (embedded Postgres), the same run before and after marking:
waited round trips per call, and on the cache the reads it answered from memory and its early flushes per call. "Then
reads through" is a service whose frame a `no-tx-cache` service (Mantle's asset reservations) turned to read-through.

| Service | Calls | Round trips off | On | Fewer | Reads from memory | Early flushes |
|---|---:|---:|---:|---:|---:|---:|
| `ledger.SetupServices.check#Readiness` | 8 | 449 | 216 | 52% | 233 | 0.0 |
| `products.CatalogServices.find#ProductPrices` | 13 | 218 | 28 | 87% | 190 | 0.0 |
| `customerservice.CustomerServices.get#Customer360` | 8 | 646 | 473 | 27% | 173 | 0.0 |
| `products.MethodServices.get#BomWhereUsed` | 5 | 214 | 42 | 81% | 173 | 0.0 |
| `account.PaymentRunServices.apply#RunPayment` | 7 | 312 | 151 | 52% | 119 | 0.0 |
| `party.AgreementServices.release#PurchaseAgreement` | 9 | 172 | 65 | 62% | 99 | 0.0 |
| `projects.ProjectServices.get#ProjectsHome` | 9 | 389 | 299 | 23% | 87 | 0.0 |
| `order.PurchaseServices.store#PurchaseOrderItem` | 22 | 152 | 63 | 59% | 82 | 0.0 |
| `work.RoutingServices.release#RoutingDraft` | 28 | 256 | 168 | 34% | 51 | 0.0 |
| `order.ShopCheckoutServices.submit#Checkout` | 29 | 162 | 89 | 45% | 32 | 0.0 |
| `order.ShopCheckoutServices.begin#Checkout` | 22 | 115 | 43 | 62% | 71 | 0.0 |
| `product.BomServices.release#BomDraft` | 52 | 156 | 89 | 43% | 48 | 0.0 |
| `account.PaymentRunServices.pay#RunSupplier` | 12 | 145 | 79 | 46% | 48 | 0.2 |
| `work.ProductionServices.create#ProductionOrder` | 106 | 300 | 238 | 21% | 13 | 0.0 |
| `account.InvoiceServices.change#InvoiceState` | 72 | 152 | 91 | 40% | 43 | 0.0 |
| `product.CostServices.preview#ProductCostRollup` | 16 | 118 | 59 | 50% | 52 | 0.0 |
| `ledger.InvoiceAutoPostServices.repost#Invoice` | 217 | 132 | 78 | 41% | 33 | 0.0 |
| `order.ShopCheckoutServices.set#CheckoutContact` | 20 | 144 | 93 | 35% | 45 | 0.1 |
| `order.ShopCheckoutServices.select#Payment` | 19 | 86 | 39 | 55% | 47 | 0.0 |
| `products.MethodServices.get#MethodContext` | 44 | 80 | 32 | 59% | 47 | 0.0 |
| `product.SupplyPlanningServices.convert#PlanningSuggestion` | 30 | 113 | 69 | 39% | 34 | 0.0 (then reads through) |
| `ledger.SetupServices.store#SetupRun` | 17 | 70 | 33 | 53% | 5 | 0.1 |
| `order.ShopCheckoutServices.get#Checkout` | 29 | 71 | 34 | 52% | 37 | 0.0 |
| `order.OrderServices.add#OrderProductQuantity` | 234 | 97 | 60 | 38% | 30 | 0.2 |
| `work.ProductionServices.record#ProductionQuantity` | 23 | 205 | 168 | 18% | 37 | 0.1 (then reads through) |
| `product.SupplyPlanningServices.run#SupplyPlan` | 31 | 124 | 87 | 30% | 23 | 0.0 |
| `work.ProductionServices.transition#ProductionOrder` | 312 | 101 | 70 | 30% | 19 | 0.4 |
| `request.SupportCaseServices.create#Case` | 66 | 67 | 38 | 43% | 24 | 0.2 |
| `work.RoutingServices.store#RoutingOperation` | 32 | 102 | 74 | 28% | 15 | 0.0 |
| `order.OrderServices.place#Order` | 202 | 105 | 78 | 25% | 22 | 1.0 (then reads through) |
| `products.CatalogServices.get#ProductDetail` | 58 | 48 | 22 | 53% | 26 | 0.0 |
| `forum.DeliveryWorkerServices.prepare#Outbox` | 46 | 72 | 48 | 34% | 21 | 0.0 |
| `order.OrderServices.approve#Order` | 183 | 81 | 58 | 29% | 19 | 0.9 (then reads through) |
| `product.ShopFulfillmentServices.run#FulfillmentStep` | 83 | 50 | 27 | 46% | 17 | 0.0 |
| `org.moqui.impl.CallServices.start#Call` | 3 | 49 | 28 | 44% | 11 | 0.0 |
| `product.BomServices.store#Bom` | 46 | 70 | 50 | 28% | 10 | 0.0 |
| `order.ReturnPolicyServices.authorize#Return` | 42 | 53 | 34 | 37% | 14 | 0.0 (then reads through) |
| `product.BomServices.store#BomItem` | 73 | 77 | 57 | 25% | 8 | 0.0 |
| `order.ShopCheckoutServices.run#Step` | 136 | 53 | 34 | 36% | 16 | 0.2 (then reads through) |
| `products.CatalogServices.store#ProductPrice` | 136 | 78 | 60 | 23% | 9 | 0.0 |
| `org.moqui.impl.VoiceServices.receive#PhoneCall` | 1 | 33 | 16 | 52% | 7 | 0.0 |
| `account.CustomerFinanceServices.get#CustomerFinance` | 48 | 54 | 38 | 30% | 16 | 0.0 |
| `account.InvoiceServices.store#InvoiceDraftItem` | 91 | 39 | 24 | 39% | 14 | 0.0 |
| `account.InvoiceServices.create#Receivable` | 80 | 46 | 31 | 32% | 9 | 0.0 |
| `products.PriceNoticeServices.send#ProductPriceNotice` | 1266 | 23 | 17 | 26% | 6 | 0.0 |

All 45: 319883 waited round trips with the cache off, 210973 on (34% fewer).

With 200 µs to the database, each waited round trip saved is 0.2 ms: `check#Readiness` 47 ms less a call,
`find#ProductPrices` 38 ms, `place#Order` 5 ms, `send#ProductPriceNotice` 1.2 ms.

Also:
1. **A held time stamp reads back at the precision its database keeps**: Postgres the nearest microsecond (half to
   even, as its `rint`), MySQL `DATETIME(n)` six digits cut then half up to n (`Dialect::timestamp_digits`).
2. **The census leftovers are not the cache's**: Consent, Contacts and CustomerServiceAccess (four features) fail with
   the cache off too in a full run of mr-boss (they depend on state earlier specs leave); WorkCenters passes now.
   ShopCart (two) and SupportAccess (one) fail when Mantle's cart and link specs and mr-pass's fall in one ten-minute
   bucket of `record#Attempt`'s rate limit (one requester for every spec): they pass with mr-pass run alone, cache or
   not. `DataGrid` under the census is `save#DataGridChanges`, which runs without the cache by design.
3. A census line is one write, so lines from several threads do not split each other.

Next:
1. The view reads that flush `place#Order` and `approve#Order` (`check#OrderPreApprove`'s `OrderItemAndPart`,
   `handle#OrderMajorChange`'s `OrderItemBillingDetail`) and the biggest flushers (`refresh#Mode`'s
   `ForumInternalAccount`, `update#ProjectTimeFromTask`'s `TaskTimeSummary`): a framework way to answer a view read in
   the frame when the held member rows cannot change its joins (updates of fields the view neither joins nor
   conditions on, read as the overlay of the view's rows), else the service reads the member entity.
2. Mantle's `no-tx-cache` asset reservations (742 early flushes, every order placement): Java took the cache off for
   their locks; with `for-update` finds going to the database under hold, check whether they can hold.
3. The shared services next (`create#Payment`, `apply#Settlement`, `pack#Shipment`, `ship#OrderPart`), by their callers.

## Round 24: view reads see held rows without writing them

Round 23's first two next steps.

1. **A view read shadows the member tables the frame holds rows of** (`moqui-db` `TableShadow`, `tx::view_shadows`): on
   Postgres the member table is read as `(SELECT columns FROM table WHERE NOT (key of each held row) UNION ALL SELECT
   CAST(? AS type), ...)`, the held rows' values as the frame sees them (held creates, and stored rows with the held
   updates on them; held deletes only hidden). The database joins, filters, groups and counts what the frame would see,
   outer joins, aggregates, sub-select and nested-view members included, and nothing held is written before the commit.
   Only members with a held row that may be in the result are shadowed (D5's test). It falls back to the early flush
   on another database, for a member with an encrypted or full-text field, and past 500 held rows of the members. The
   unit tests' SQL engine (`testing.rs`) reads shadows too.
   Census 7, every component's specs with the cache everywhere: view-read early flushes 3,652 to 34 (4,779 view reads
   shadowed), the same features passing, no commit batch failing.
2. **Mantle's 19 `no-tx-cache` services hold** (the asset reservations, issuances, receipts, physical inventory, and
   `cancel#OrderItem`, `reject#OrderItem`): each locks its `Asset` (or `OrderItem`) rows with a `for-update` find,
   which under hold goes to the database and keeps its lock to the commit, and reads the rows it changed through views,
   which are shadowed now. `place#Order` and `approve#Order` no longer turn to read-through.

The 45 services of Round 23, waited round trips per call in the specs of mr-mantle, mr-boss and mr-pass (the 20 that
changed most from Round 23):

| Service | Calls | Off | Round 23 | Round 24 | Fewer than off | Early flushes | Shadowed reads |
|---|---:|---:|---:|---:|---:|---:|---:|
| `work.ProductionServices.record#ProductionQuantity` | 23 | 205 | 168 | 128 | 37% | 0.0 | 0.7 |
| `order.OrderServices.place#Order` | 202 | 105 | 78 | 71 | 33% | 0.0 | 1.5 |
| `account.PaymentRunServices.pay#RunSupplier` | 12 | 145 | 79 | 74 | 49% | 0.0 | 0.3 |
| `order.OrderServices.approve#Order` | 183 | 81 | 58 | 54 | 33% | 0.0 | 0.0 |
| `order.ShopCheckoutServices.run#Step` | 136 | 53 | 34 | 31 | 42% | 0.0 | 0.6 |
| `work.ProductionServices.transition#ProductionOrder` | 312 | 101 | 70 | 68 | 33% | 0.0 | 0.4 |
| `product.SupplyPlanningServices.convert#PlanningSuggestion` | 30 | 113 | 69 | 66 | 41% | 0.0 | 0.0 |
| `order.ShopCheckoutServices.submit#Checkout` | 29 | 162 | 89 | 86 | 47% | 0.0 | 0.0 |
| `order.OrderServices.add#OrderProductQuantity` | 234 | 97 | 60 | 59 | 40% | 0.0 | 0.3 |
| `request.SupportCaseServices.create#Case` | 66 | 67 | 38 | 37 | 45% | 0.0 | 0.3 |
| `order.ReturnPolicyServices.authorize#Return` | 42 | 53 | 34 | 33 | 38% | 0.0 | 0.0 |
| `product.CostServices.preview#ProductCostRollup` | 16 | 118 | 59 | 58 | 51% | 0.0 | 0.0 |
| `ledger.SetupServices.store#SetupRun` | 17 | 70 | 33 | 32 | 54% | 0.0 | 0.1 |
| `org.moqui.impl.CallServices.start#Call` | 3 | 49 | 28 | 27 | 45% | 0.0 | 0.0 |
| `product.BomServices.store#Bom` | 46 | 70 | 50 | 50 | 29% | 0.0 | 0.0 |
| `order.PurchaseServices.store#PurchaseOrderItem` | 22 | 152 | 63 | 63 | 59% | 0.0 | 0.0 |
| `ledger.InvoiceAutoPostServices.repost#Invoice` | 217 | 132 | 78 | 78 | 41% | 0.0 | 0.0 |
| `order.ShopCheckoutServices.set#CheckoutContact` | 20 | 144 | 93 | 93 | 35% | 0.0 | 0.1 |
| `work.RoutingServices.store#RoutingOperation` | 32 | 102 | 74 | 74 | 28% | 0.0 | 0.0 |
| `products.PriceNoticeServices.send#ProductPriceNotice` | 1266 | 23 | 17 | 17 | 27% | 0.0 | 0.0 |

All 45: 319883 waited round trips off, 210973 in Round 23 (34% fewer), 205714 now (36% fewer).

Every one of the 45 now commits in one batch, with no early flush.

Next:
1. The view reads left that flush: `PartyDetail` (Person's encrypted `mothersMaidenName`: a shadow could carry the held rows' sealed values) and views built in code (data documents,
   REST query views: `flush_for_members`), which could take the same shadows.
2. `distinct`, `having` and aggregate finds of one entity (66 early flushes in Census 7, most of them `pack#Shipment`'s
   `ShipmentItemSource`): the same shadow as a sub-select of the entity's table.
3. The shared services (`create#Payment`, `apply#Settlement`, `pack#Shipment`, `ship#OrderPart`), by their callers.

## Round 25: distinct reads and encrypted members shadowed, the shared Mantle services on the cache

Round 24's next steps, after merging the latest mr-mantle (a conflict on `run#Step` and `run#FulfillmentStep`, where
upstream added `operation` and this branch `transaction="cache"`: both kept) and mr-boss.

1. **A `distinct` or `having` find or count of one entity** the frame holds rows of reads a shadow of its table too
   (`tx::entity_shadows`; `moqui-db` `write_table` now serves single-entity selects and counts as well as views, the
   shadow aliased by the table's own name), so `pack#Shipment`'s and `handle#OrderMajorChange`'s distinct reads no
   longer write early. One `read_shadows` decides for `find::plan`, after a check that this thread holds anything.
   Where no shadow can be made (a full-text field) the held prefix is written first, as before.
2. **An encrypted member is shadowed** with its rows as stored: each held row sealed (`field_rw::seal_stored`). Only
   an encrypted primary key still falls back. `PartyDetail` (Person's `mothersMaidenName`) no longer writes early.
3. **The shared services on the cache**: `apply#Settlement` (no service calls it), `ship#OrderPart` (only the test
   order generator), `pack#Shipment` (from `ship#OrderPart` and `complete#ShipmentPack`) and `create#Payment`, whose
   nine service callers (refunds, internal transfers, financial account disbursements, order and return payments)
   are payment operations themselves, all held under the census with every spec passing.

Round trips per call in the specs of mr-mantle, mr-boss and mr-pass, the four new services first:

| Service | Calls | Off | Round 24 | Round 25 | Fewer than off | Early flushes | Shadowed reads |
|---|---:|---:|---:|---:|---:|---:|---:|
| `account.PaymentServices.apply#Settlement` (new) | 30 | 242 | 242 | 122 | 49% | 0.0 | 1.4 |
| `shipment.ShipmentServices.pack#Shipment` (new) | 44 | 208 | 208 | 167 | 19% | 0.0 | 0.5 |
| `account.PaymentServices.create#Payment` (new) | 156 | 48 | 48 | 28 | 42% | 0.0 | 0.0 |
| `shipment.ShipmentServices.ship#OrderPart` (new) | 20 | 395 | 244 | 234 | 41% | 0.0 | 2.7 |
| `order.ShopCheckoutServices.run#Step` | 172 | 53 | 31 | 28 | 48% | 0.0 | 0.6 |
| `product.ShopFulfillmentServices.run#FulfillmentStep` | 92 | 50 | 27 | 25 | 49% | 0.0 | 0.0 |
| `products.CatalogServices.store#ProductPrice` | 136 | 78 | 60 | 59 | 24% | 0.0 | 0.0 |
| `order.ShopCheckoutServices.select#Payment` | 23 | 86 | 39 | 38 | 56% | 0.0 | 0.0 |
| `order.ShopCheckoutServices.begin#Checkout` | 26 | 115 | 43 | 43 | 63% | 0.0 | 0.0 |
| `party.AgreementServices.release#PurchaseAgreement` | 9 | 172 | 65 | 65 | 62% | 0.0 | 0.0 |
| `work.ProductionServices.record#ProductionQuantity` | 23 | 205 | 128 | 128 | 38% | 0.0 | 0.7 |
| `product.BomServices.release#BomDraft` | 52 | 156 | 89 | 89 | 43% | 0.0 | 0.0 |
| `product.SupplyPlanningServices.convert#PlanningSuggestion` | 30 | 113 | 66 | 66 | 41% | 0.0 | 0.0 |
| `order.ReturnPolicyServices.authorize#Return` | 42 | 53 | 33 | 33 | 39% | 0.0 | 0.0 |

The 45 of Round 23: 324548 waited round trips off, 208234 in Round 24 (36% fewer), 207658 now (36% fewer).

Census 8 (mr-mantle, mr-boss and mr-pass with the cache everywhere): 145,367 transactions, 19 early flushes (913 in
Census 7 over every component, 4,482 in Census 6), 5,697 reads shadowed, no commit batch failing, the same features
passing. The 19: 15 `PartyDetail` reads inside `get#DuplicatePairs`, a frame of 2,032 held updates (past the 500 a
shadow takes, so written first by design), and 4 of the `MoquiChatEvent` data document (`index#ChatEvent`), a view
built in code (`flush_for_members`).

`cargo test --workspace` failed on moqui-core's `tarpit_locks_and_check_views_are_read_once_until_a_lock_is_written`
for the second time (Round 22 too): the authorization generation is a process static that other tests move, so a
lock read between its hits can repeat. The test now allows for the moves of other threads (`moved_here`).

`ProductsMethods` (mr-boss) ran 15.8 s in `./moq test mr-mantle mr-boss mr-pass`, over the 15-second budget: it takes
2.6 s alone and 5 s in `./moq test-apps`, and every one of its features is slower after Mantle's specs (screen renders
whose cost grows with the data earlier specs leave). Its time is the data's, not the cache's; not changed here.

Next:
1. Data documents and REST query views on the same shadows (`flush_for_members`).
2. A census of the specs with the 49 services on the cache and no census mode, to find the services left whose
   transactions read most (the next set to mark), now that nothing they do flushes early.
3. The external-Postgres timing of the Mantle operations (`place#Order`, `apply#Settlement`) with 200 µs of delay, for
   the time a call saves rather than its round trips.

## Round 26: views built in code shadowed, 13 more services, the Mantle operations timed with 200 µs to the database

Round 25's next steps, after the last Commerce and Shop merges of mr-moqui, mr-boss, mr-pass and mr-mantle.

1. **A view built in code** (a data document, a REST query view: `query_rest::run_view`) reads the frame's held rows
   of its members through shadows (`tx::member_shadows`), as a view entity does, instead of writing them first.
   Census 9, every component's specs with the cache everywhere: 152,290 transactions, 42 early flushes, 5,706 reads
   shadowed, no commit batch failing. The 42: 27 transactions past the 10,000 held writes of the size cap (data
   loads, no service), and the 15 `PartyDetail` reads of `get#DuplicatePairs`' frame of 2,032 updates (past the 500 a
   shadow takes). No view built in code flushes now.
2. **13 more services on the cache**, the next of Census 9's ranking that hold cleanly: six no service calls
   (`add#CartItem`, `attach#Cart`, `record#ReturnSettlement`, `apply#InboundDocument`, `approve#Payable`,
   `convert#PurchaseSourcingGroup`) and seven whose one caller is an operation itself (`issue#ProductionMaterial` and
   `receive#ProductionOutput` from `receive#SubcontractOutput`, `execute#PaymentGatewayAction` and
   `refund#ReviewedPayment` from `confirm#PaymentReview`, `receive#ShipmentStock` from `receive#ExpectedShipment`,
   `get#PaymentFinanceDetail` from a payment screen service, `release#ProductionOrder` from a demo loader).
3. **A `service-call` with `transaction="force-new"` runs a `cache` service without the cache**: the XML actions
   template writes `.useTransactionCache(false)` for any `transaction` but `cache` and `force-cache`, as Java's did.
   Half of `pack#Shipment`'s calls came so, from `complete#ShipmentPack`; it and Mr. Boss's Customer 360 screen call
   now say `force-cache` (a transaction of its own, with the cache). The guide says so.
4. **The Mantle operations timed against an external Postgres with 200 µs of added delay** (the docker server of
   round 5, the network delay by `tc netem`): every component's data loaded into a database there, copied per run
   from it as a template, and the order, accounting and warehouse specs of mr-mantle run on it with `--db existing`
   (`OrderToCashBasicFlow`, `OrderProcureToPayBasicFlow`, `AccountingSettlement`, `AccountingPaymentLifecycle`,
   `AssetReservationMisc`, `WarehouseShipping`, `AccountingPayables`, `InvoiceOrderApply`; 111 features, all passing
   both ways), once with `-Dentity_tx_cache=false` (new: the transaction cache off everywhere) and once with it on.
   Each transaction's time is in its census line now (`micros`, begin to the end of the commit). Median per call:

| Service | Calls | ms off | ms on | Faster | Round trips off | On |
|---|---:|---:|---:|---:|---:|---:|
| `shipment.ShipmentServices.pack#Shipment` | 16 | 68.3 | 44.9 | 34% | 263 | 138 |
| `account.PaymentServices.apply#Settlement` | 8 | 77.0 | 60.4 | 22% | 311 | 154 |
| `account.InvoiceServices.approve#Payable` | 6 | 44.9 | 30.3 | 33% | 106 | 64 |
| `account.PaymentServices.create#Payment` | 11 | 37.3 | 24.6 | 34% | 111 | 66 |
| `order.OrderServices.add#OrderProductQuantity` | 32 | 26.6 | 17.5 | 34% | 98 | 59 |
| `account.InvoiceServices.change#InvoiceState` | 2 | 33.8 | 25.9 | 23% | 118 | 79 |
| `order.OrderServices.place#Order` | 26 | 29.6 | 22.0 | 26% | 107 | 71 |
| `order.OrderServices.approve#Order` | 26 | 24.1 | 17.4 | 28% | 79 | 53 |
| `account.PaymentServices.get#PaymentFinanceDetail` | 24 | 18.9 | 15.5 | 18% | 74 | 38 |

Time in these 9 services' transactions: 6037 ms off, 4126 ms on (32% less).

Next:
1. The bulk operations past the shadow's 500 held rows (`get#DuplicatePairs`): a larger cap, or a shadow sent as
   a temporary table, for a frame that holds thousands.
2. The time of a held call that is not its round trips: `pack#Shipment` and `apply#Settlement` spend 45-60 ms with 138
   and 154 round trips (28-31 ms of the delay), so the rest is local work; a profile of them on the external server.
3. The next services of the ranking, and screen transitions' and REST calls' own transactions (the service each
   starts with is often not marked, and holds nothing).

## Round 27: large shadows as arrays, a profile of the Mantle operations, and the script engine's compile and heap check

Round 26's next steps.

1. **A shadow of more than 16 rows goes to Postgres as arrays**, one parameter a column: `(k1, k2) NOT IN (SELECT *
   FROM unnest(CAST(? AS type[]), ...))` for the hidden keys and `UNION ALL SELECT * FROM unnest(...)` for the rows
   (`statement::write_table`, the values as their text parameters are sent, quoted in an array literal). The statement's
   text and parameter count no longer grow with the rows, so the shadow takes up to 10,000 held rows (it was 500, against
   Postgres's 65,535 parameters), and a bulk frame such as `get#DuplicatePairs`' 2,032 updates is read without writing
   first. Tested on the embedded Postgres with 43 held rows, quotes, a backslash, braces and a null.
2. **A CPU profile of the Mantle operations** (`perf record -g` of a frame-pointer release build running
   `AccountingSettlement`, `AccountingPaymentLifecycle`, `WarehouseShipping` and `OrderToCashBasicFlow`; samples under a
   service call): the time a held call spends beyond its round trips is the script engine's, not the cache's.
   - `JS_ComputeMemoryUsage` 4.0%: the check of the engine's heap against the cache-pressure limit walked every object
     of the heap, every 16th script. An engine now runs QuickJS on a `CountingAllocator` (rquickjs's `RustAllocator`,
     its live bytes counted), so the check reads a number, on every script.
   - Compiling JS 15.5%: an engine keeps its compiled scripts, but each nesting depth and thread has its own engine, and
     the 9.7 MB of Mantle and Mr. Boss JS is more than one engine's 4 MiB cache, so the same modules were parsed again
     and again. The bytecode of a compiled script is kept for the process now (`compiled::SHARED_BYTECODE`, 64 MiB):
     another engine reads it (`JS_ReadObject`) instead of parsing, the host scripts an engine installs when it starts
     included (`eval_js`).
   - SQL redaction 1.6%, twice a statement (the query stats' key and a traced span's text): kept per thread by the text.
   The same specs after: 8,729 samples under service calls against 10,196 (14% less CPU), compiling 9.8% (it was
   15.5%; most of what is left is new engines' first scripts), `JS_ComputeMemoryUsage` and the redaction gone. The
   moqui-core unit tests run in 33 s (42 s).
3. **Screen transitions on the cache, tried and left off**: `screen_tx_cache` (new) makes a transition's transaction,
   and a screen's with `begin-transaction`, begin as `cache`. With it on, Mr. Boss's `WarehouseReceivingWalk` (Finish
   receipt) fails every time in the client suite: the receipt page after `closeRemainder` shows the shipment still open,
   though the transition's message says it was delivered. It passes alone and with the setting off, so the default
   is `false` until that is understood. The census does not run the client walks, so it did not see this.

The Mantle operations against the Postgres 200 µs away, as in Round 26 (median ms a call):

| Service | Calls | ms off | Round 26 | Round 27 | Faster than off |
|---|---:|---:|---:|---:|---:|
| `order.OrderServices.place#Order` | 26 | 29.6 | 22.0 | 21.7 | 27% |
| `order.OrderServices.approve#Order` | 26 | 24.1 | 17.4 | 17.3 | 28% |
| `order.OrderServices.add#OrderProductQuantity` | 32 | 26.6 | 17.5 | 17.6 | 34% |
| `account.InvoiceServices.change#InvoiceState` | 2 | 33.8 | 25.9 | 23.6 | 30% |
| `account.PaymentServices.create#Payment` | 11 | 37.3 | 24.6 | 23.5 | 37% |
| `account.PaymentServices.apply#Settlement` | 8 | 77.0 | 60.4 | 51.1 | 34% |
| `shipment.ShipmentServices.pack#Shipment` | 16 | 68.3 | 44.9 | 45.0 | 34% |
| `account.PaymentServices.get#PaymentFinanceDetail` | 24 | 18.9 | 15.5 | 13.7 | 27% |
| `account.InvoiceServices.approve#Payable` | 6 | 44.9 | 30.3 | 31.7 | 29% |

All of them: 6037 ms off, 4126 ms in Round 26, 3954 ms now (35% less than off).

Next:
1. Why the receiving walk reads a stale shipment with transitions held (`close#ReceiptRemainder` under a held
   transition: its nested `force-new` work against the transition's held writes of the same rows), then the
   transitions on the cache by default; and the census over the client walks too.
2. The rest of the profile: SipHash 5% (the compiled scripts' text hash, the telemetry histogram's attribute hash,
   the statement cache's key), allocation 12% (`malloc` and `free`), `memcmp` 3% (script text compared on a cache hit).
3. `WorkCenters`' load of a released operation, which failed once in a full `./moq test-apps` (and once under the
   census in Round 22) and passes alone and in the full mr-mantle run.

## Round 28: one script engine per thread, and inert modules kept for the run

Round 27 left the script engine's memory and its repeated work. A baseline of the full mr-mantle regression run, by
process-wide counts (`script::engine_stats`, written per process to `MOQUI_SCRIPT_STATS_FILE` at the end of `moqui
test`), and a spike of what QuickJS allows (`script/runtime/realm_tests.rs`):

- 104 threads ran scripts in 588 engines (a runtime for each nesting depth a script reached: `Context::with` cannot be
  entered again while held), up to 222 alive at once, nesting 18 deep; the JS heaps took up to 393 MiB.
- A runtime costs 27 KiB and a context 52 to 76 KiB, but each copy of a script's functions about 2.6 times its
  bytecode. A function compiled in one context runs with that context's globals from any other (QuickJS keeps its
  realm), so compiled code is not shared between contexts; bytecode read into each is. A context per depth would save
  little: what costs is a copy of each module per engine.
- A host call of a running script can enter its runtime again with `Ctx::from_raw` while the outer `with` holds the
  lock, in the same context or a sibling one, three deep, an exception coming back cleanly.

Built:

1. **One engine per thread.** A script that runs while another runs on the thread (a service a script calls runs a
   script) runs in the same engine and context (`with_nested_context`), entered again through the outer lock. Its run
   state is its own: the caller's tracked and released values, screen unit depth and flush in progress are set aside
   untouched and given back after (`OuterRun`), as a runtime of its own left them. A script file and generated actions
   are already functions, so their declarations do not meet; a screen mode's outermost unit now saves the render state
   around it and gives it back (`enter`/`leave` in ScreenHtml.js, ScreenQvt.js, ScreenQvtStatic.js), so a document
   rendered inside a render (an html-print a service makes) leaves the outer render as it was. The stack limit is set
   once, at the outermost entry, for the scripts it nests. The test harness's spec code runs in an engine of its own
   (`EngineKind::Spec`): the globals it defines (`call`, `login`, ...) would otherwise come before the context stack for
   the services a spec calls.
2. **An inert module is kept for the outermost run.** `ec.resource.script` ran a module's whole top level at every
   load, making its functions again (5.5% of a Mantle service call's CPU). A module whose top level only declares
   (functions, constants of literals, tables of them, other modules, the module's own names and operators over them:
   `parse::module_is_inert`, decided once per file text) gives the same value for the rest of the outermost script run
   on the thread, and is made again in the next. 290 of the 327 loaded modules of the runtime are inert; the others
   keep a memo at the top level (`let accessCache = null`), change a table at load, or build a constant with a call,
   and are made at each load as before. `script_module_cache=false` turns it off.

The full mr-mantle run after (1,908 of 1,908 passing):

| | before | after |
|---|---:|---:|
| engines started | 588 | 155 (104 main, the rest the harness's spec engines) |
| alive at once, at most | 222 | 32 |
| JS heap, all engines, at most | 393 MiB | 139 MiB |
| JS heap at the end | 110 MiB | 25 MiB |
| scripts read from the shared bytecode | 23,537 | 12,139 |
| feature time | 224 s | 213 to 215 s |

Found on the way, not this round's:
- `MantleSearch` ("the sales fixture loads and its feed is indexed") and `SalesReportStoreScope`'s setup fail in a run
  that starts an hour or more after its test database snapshot was made: the demo data load leaves data feed pending
  rows whose document ids lack key values (`MantleSalesOrderPart` `101190`, `MantleSalesOrderItem`,
  `MantleInventoryRunConsume`, `MantleInventoryRunProduce`), which never deliver and count as stuck after 60 minutes.
- `AccountingCollections` (an instalment's overdue days, 9 for 10) and `SupportCases` (Today) fail after 23:00 local
  time, on Round 27's code too.
- moqui-core's tarpit lock test counted other threads' generation moves from after its first hit; a move during that
  hit's load is counted now.

Next:
1. The data feed's pending rows with partial document ids (above).
2. The rest of the profile: SipHash 5%, allocation 12%, `memcmp` 3%.
3. The receiving walk with transitions held (`screen_tx_cache`), and `WorkCenters`' intermittent load count.

## Round 29: the data feed's related writes without values, and fast hashes for the internal maps

Round 28's next steps.

1. **A write recorded without values, of an entity a document reads through a relationship, is fed by the
   document's primary rows.** A data load writes rows before the feed configuration has been read, so their touches
   keep no values and the commit finds the rows through the Postgres `xmin` probe. That path wrote a pending row for
   every document on the entity with the written row's own key, which for a related entity (an `OrderHeader` of
   `MantleSalesOrderPart`) is not a document id: delivery could never read it (`document id 101190 does not have 2 key
   values`), and after 60 minutes the rows counted as stuck, so `MantleSearch` and `SalesReportStoreScope` failed in
   any run started an hour or more after its test database snapshot. The probe path now reads the row and walks to
   the document's primary rows, as a write with values does
   (`a_related_write_without_values_is_fed_by_the_documents_primary_keys`). A fresh data load of mr-mantle made 12 such
   rows before and none after.
2. **foldhash for the hashes of the internal maps keyed by text** (the Postgres statement cache by SQL, the query
   stats' key memo and shards, the redaction memo, the transaction frames by token, the compiled scripts by text), and
   a script file's text hashed once when it is read rather than at each `ec.resource.script`. Already in the tree under
   hashbrown. The profile of the Mantle specs: 7,564 samples under service calls against 8,365 (10% less), SipHash 3.2%
   of them against 5.0% (what is left is OpenTelemetry's attribute sets and `IndexMap`s of values).
3. **The receiving walk** (`WarehouseReceivingWalk`) failed once in `./moq test-apps` with `screen_tx_cache` off too,
   and passes in the Mr. Boss client suite alone with it on: it is flaky by itself, more often with transitions held.
   Its cause is not found; the setting stays off.

Seen in the runs: moqui-host's xmpp test failed twice right after a full run and passed 8 of 8 after; moqui-core's
`calls_are_counted_only_between_enable_and_finish` failed once in a full run and passed in 6 more.

Next:
1. The receiving walk: a log of its transitions' transactions (the census over the client walks) to find what is
   stale, then transitions on the cache.
2. Allocation (13% of the service calls' time: `malloc` and `free`) and `memcmp` (3.7%: string compares in scripts'
   `every`, select lists, the telemetry span's secret check).
3. OpenTelemetry's attribute set hashing on each metric record (1%).

## Round 30: screen transitions on the cache, the receiving walk's race, and snapshots by day

Round 29's next steps.

1. **The receiving walk's failure was the walk's own race, not stale data.** The census over the Mr. Boss client
   walks with transitions held showed `closeRemainder`'s transaction holding and committing cleanly, and the failed
   page's saved text shows the receipt delivered with no "Finish receipt" on it: the walk asserted the dialog gone a
   moment before it closed (its "Delivered" wait is met by the transition's message). Holding the transition's writes
   to the commit made the race likelier. The walk now waits for the dialog to go (mr-boss); the client suite passes 3
   of 3 with transitions held.
2. **Screen transitions (and `begin-transaction` screens) begin as `cache` by default** (`screen_tx_cache`, true;
   false begins them as `use`). `./moq test-apps` client walks 17 of 17 with it, `./moq test-framework` every step.
3. **A data document's code view reads only the names it needs** of its member entities, not a snapshot of each
   (fields, relationships, masters): 2.8% of the Mantle specs' service calls. A telemetry span's check of an attribute
   name for a secret is kept per thread by name. The profile after: 7,390 samples under service calls (7,564 in Round
   29).
4. **Test snapshots are of a day.** `Tasks`' smart lists (a task due today), and likely `SupportCases`' Today and
   `AccountingCollections`' overdue days, fail on a snapshot loaded the day before: the demo data is dated from its
   load. The snapshot key now has the local day, and a snapshot unused for two days is removed when one is saved. Not
   yet seen in a run: `./moq test-apps` did not rebuild the binary (it ran the one built before this round), so the
   runs here restored yesterday's snapshot; `runtime/save/test-*` were removed by hand.

Next:
1. A `./moq test-apps` on a rebuilt binary, to see the day-keyed snapshots and `Tasks` pass; and why `test-apps`
   kept a binary older than its sources.
2. Allocation (still the largest: `malloc` and `free`) and `memcmp` in the service calls.

## Round 31: quickjs-oxide for small scripts (Script-Engine-Oxide Phases 1 and 2)

[Script-Engine-Oxide](Script-Engine-Oxide.md) runs conditions, values, `${}` text, maps and the subset of XML actions
on quickjs-oxide under `script_engine=small-oxide`. Measured on a quiet machine with release builds; the default stays
`rquickjs` because the speed gate missed.

| Row (µs a call) | QuickJS | oxide |
|---|---|---|
| `service_speed` `echo#Data` | 11.5–11.8 | 13.7–14.1 |
| `js_speed` value `a` | 0.39 | 0.43 |
| value `a + b * 2` | 0.56 | 0.65 |
| value `item.name` | 1.23 | 1.37 |
| value `{x: a, y: item.name}` | 1.98 | 3.12 |
| value `price.multiply(dqty)` | 1.31 | 1.63 |
| value `itemList.length` (100 maps) | 57.4 | 76.9 |
| condition | 0.59 | 0.85 |
| expand `Item ${...}: ${...}` | 1.34 | 1.81 |
| value `a + N`, new text each call | 0.55 | 1.23 |

- The bindings allocate on each call where QuickJS's do not (4 allocations for `a`, 53 for a new map, 1,319 for 100
  maps' length) and a new text costs a check and an oxide compile. Those are the next work (F4 in the fork, and
  conversions that keep a value's wrapper).
- Heap: the full `./moq test-apps` run peaked at 138.4 MB of JS heap under `small-oxide` (44.0 MB of it oxide) and
  139.6 MB under `rquickjs`, against Round 28's 139 MiB.

## Round 32: everything on quickjs-oxide (Script-Engine-Oxide Phase 3)

[Script-Engine-Oxide](Script-Engine-Oxide.md) Phase 3 runs every kind of script on quickjs-oxide under
`script_engine=oxide`. The fork is at 28aede7d (8635ecb5 since, with one error-path fix), with its F4 speed work. Release builds on a quiet machine; the default
is still `rquickjs`.

| `js_speed` row (µs a call) | QuickJS | oxide | ratio |
|---|---|---|---|
| empty script | 0.20 | 0.16 | 0.80 |
| 1000 adds to a local | 35.9 | 8.0 | 0.22 |
| 1000 adds to a bare name | 415 | 376 | 0.91 |
| walk itemList, 100 context maps | 59.4 | 195.5 | 3.29 |
| 100 `ec.user.nowTimestamp` | 30.4 | 67.6 | 2.23 |
| 10 cached entity finds | 43.4 | 60.2 | 1.39 |
| template, trusted | 59.3 | 95.8 | 1.62 |
| template, locked down | 66.6 | 89.4 | 1.34 |
| screen Plain, html | 27.9 | 28.6 | 1.03 |
| screen Labels, html | 224 | 307 | 1.37 |
| screen Single, html | 315 | 474 | 1.50 |
| screen Single.qvt | 510 | 815 | 1.60 |
| screen List, html, 100 rows | 9268 | 10846 | 1.17 |
| screen List.qvt, 100 rows | 12953 | 16277 | 1.26 |
| screen List, text, 100 rows | 2876 | 3121 | 1.09 |

The Mantle operations against the Postgres 200 µs away, as in Round 26 (median ms a call, transaction cache on, two
runs of each engine):

| Service | Calls | QuickJS | oxide | Ratio | Round trips |
|---|---:|---:|---:|---:|---:|
| `order.OrderServices.place#Order` | 26 | 21.5 | 22.1 | 1.030 | 66 |
| `order.OrderServices.approve#Order` | 26 | 16.8 | 16.9 | 1.006 | 54 |
| `order.OrderServices.add#OrderProductQuantity` | 32 | 16.9 | 17.7 | 1.051 | 52 |
| `account.InvoiceServices.change#InvoiceState` | 2 | 23.5 | 23.6 | 1.006 | 79 |
| `account.PaymentServices.create#Payment` | 11 | 23.5 | 24.4 | 1.038 | 77 |
| `account.PaymentServices.apply#Settlement` | 8 | 49.0 | 54.5 | 1.112 | 121 |
| `shipment.ShipmentServices.pack#Shipment` | 16 | 45.1 | 45.4 | 1.006 | 118 |
| `account.PaymentServices.get#PaymentFinanceDetail` | 24 | 13.5 | 14.4 | 1.062 | 27.5 |
| `account.InvoiceServices.approve#Payable` | 6 | 30.1 | 32.0 | 1.061 | 84 |

- **The Mantle operations:** median × calls is 3458 ms on QuickJS and 3594 ms on oxide (+3.9%). Round 27 was 3954 ms
  on QuickJS.
- **The cost is in the bindings, not the engine:** plain JavaScript is faster on oxide. The rows that lose are host
  calls and context reads, which allocate 4 to 10 times what QuickJS's bindings do. Narrowing that, with the fork's
  API grown to fit, is the next work.
- **Heap:** the peak of each `./moq test-apps` worker's script heaps is 91 to 118 MB under `oxide` and 126 to 140 MB
  under `rquickjs`.

The raw data and the scripts are in `../oxide-bench`: `round32/` holds the data, and `mantle-pg/` reruns the Mantle
table (its README).

Next (plans/Script-Engine-Oxide.md §11 has the files and commands):
1. **A profile of the rows that lose on oxide**, to find where the bindings spend their time and allocations:
   - a host getter: 100 `ec.user.nowTimestamp`;
   - walking 100 context maps;
   - decimals;
   - cached entity finds;
   - the trusted template;
   - form-single html and qvt;
   - `apply#Settlement` among the Mantle operations.
2. **Conversions that keep a value's wrapper**, and host calls with fewer allocations. Grow the fork's embedding API
   where the bindings need it.
3. **The heap of many runtimes:**
   - caches and atoms shared across runtimes (F1 step 3, further);
   - arenas that give back their interior free space.
4. **Then Script-Engine-Oxide Phase 4:** `oxide` the default, and rquickjs removed.

## Round 33: the bindings' first cuts, and a profile of every layer

The first round after [Script-Engine-Oxide](Script-Engine-Oxide.md) Phase 3, on `script_engine=oxide`, with no fork
change. Six changes, each measured on its own from one release build: each was behind a temporary switch
(`MOQUI_OXIDE_OFF`), and every row ran with all of them on, all off, and each one off in turn, two runs of each, with
nothing else running. The switches are gone from the commit. The raw logs, the profiles' split and the scripts are in
`../oxide-bench/round33/`.

**The changes:**
- **`leaf`.** The pure `ec.user` getters are leaves: `nowTimestamp`, `username`, `userId`, `visitId`, `getVisitId` and
  `machineTimeZone`. A call no longer lends the engine.
- **`entity`.** The entity value objects of a run are found by their cell in a map, where a scan of every value the
  run made was O(N²) over a run's rows. The entity value and list prototypes are kept on the engine.
- **`this`.** A host method lifts its receiver once; it was twice. A decimal method borrows the other decimal's value
  rather than cloning it.
- **`keys`.**
  - The literal keys the bindings read often are made once a thread: `length`, the node literals' fields and the host
    table's marker.
  - A list's `length` and index keys are read without `String`s.
  - The entity value methods are a set.
  - The map-key cache hashes with foldhash.
- **`flush`.** The write-back's comparison checks names and texts by code unit, with no `String` each.
- **`now`.** `nowTimestamp` makes its time stamp from the millis, without the conversion's text. This one is on both
  engines.

**The new `js_speed` rows** (script section): an entity list of 100 cached rows walked by index, 100 cache gets (a host
method with a text argument), and 10 noop service calls with and without `itemList` tracked.

Each change's own effect is all on against that one off. The table gives µs a call, with allocations a call in brackets:

| Row | Before (all off) | After (all on) | Mostly from |
|---|---:|---:|---|
| 100 `ec.user.nowTimestamp` | 66.7 (802) | 41.1 (702) | `leaf` −21 µs; `this` −1.7 µs and −100 allocations; `now` −1.8 µs (QuickJS 32.2 → 30.7) |
| walk an entity list, 100 cached rows | 226.7 (3966) | 191.0 (2654) | `entity` −24 µs; `keys` −12 µs |
| walk itemList, 100 context maps | 193.3 (4123) | 184.2 (3620) | `flush` −8 µs and −500 allocations |
| 10 noop service calls, itemList tracked | 402.4 (7869) | 383.1 (6824) | `flush` −17 µs |
| 10 cached entity finds | 61.9 (826) | 58.9 (716) | `entity`, `this` |
| 100 cache gets | 36.8 (711) | 34.7 (610) | `this` |
| trusted template | 92.9 (1059) | 90.8 (956) | `flush` |
| screen Single, html | 479 (5407) | 469 (4950) | `flush`, `this` |
| screen Single.qvt | 824 (9394) | 805 (8597) | `flush`, `this`, `keys` |
| screen List, html, 100 rows | 10978 (117274) | 10721 (107828) | `this` −2.3%, `keys` |
| screen List.qvt, 100 rows | 16465 (141727) | 16248 (131253) | `this`, `keys` |
| screen List, text, 100 rows | 3137 (44641) | 3051 (40959) | `this`, `keys` |

Rows that do not cross the boundary did not move: plain JavaScript, JSON, string concatenation and decimals.

**The Mantle operations** against the Postgres 200 µs away (Round 32's nine services, two runs of each engine):
- 3472 ms on QuickJS and 3550 ms on oxide, +2.2% in all (Round 32: +3.9%). The round trips are the same.
- What is left is mostly `apply#Settlement` (×1.12) and `get#PaymentFinanceDetail` (×1.10).

**The profile** (`perf record -F 4000` of the rows on the binary with every change; self time by layer, the fork's
symbols included):

| Row | Engine core | Fork API | Bindings | Framework | Allocator |
|---|---:|---:|---:|---:|---:|
| walk itemList | 36% | 16% | 5% | 5% | 23% |
| 100 `nowTimestamp` | 25% | 8% | 10% | 8% | 9% |
| walk an entity list | 23% | 12% | 8% | 17% | 19% |

It found:
- **The heap cap's check on every allocation.** Under a cap (Moqui sets one), each allocation's safepoint takes the
  slow path, which sums `Heap::bytes()` over 14 arenas and tables and `growth_bytes()` over 10. That is 2 to 5% of
  every row.
- **The fork's API re-interns each key.** `View::key` collects a `Vec<u16>`, then `AtomTable::find` and `intern_repr`
  run, on every `get_property` and `set_property`.
- **The flush of a tracked list.** It clones the stack's value before comparing: about 29 µs a flush of a 100-map list
  (10 service calls cost 383 µs with `itemList` tracked and 93 µs without). Comparing in place needs to know, without
  running code, that an object is ordinary (no Proxy, no exotic hooks). No API says so, so this moved to the fork.
- **The lend around a localized field.** It is about 11% of the entity-list walk (`lend` and `NativeCx::restore`) for
  `Enumeration.description`.

Next (Round 34, the fork's embedding API and the bindings on it), by what the profile shows:
1. **The cap's headroom kept at the slow path**, so a capped heap's allocation takes the fast path.
2. **Keys interned without a copy.** `View::key` on the Latin-1 path, keys the embedder makes once kept as atoms, and
   an index key read as a number.
3. **`is_ordinary_object`,** then the flush compared in place, with no clone.
4. **Arguments and receivers without a handle**, strings in one copy, and the released-handle buffer kept.
5. **A localized field read from the l10n cache without the lend.**

## Round 34: the fork's embedding API, and the bindings on it

Five changes, two in the fork's engine and API alone and three in the bindings on new fork API. The fork's branches
merged to its `main` (765afa3f). The bindings' changes were each behind a temporary switch for one release build.
- **`cap`** (fork, `perf/cap-headroom`): a capped heap keeps the cap's headroom at the slow path, so its allocations
  take the fast path until the headroom is used.
- **`key`** (fork, `perf/key-latin1`): `View::key` and `PropertyKey::from_js_string` intern on the Latin-1 path with
  no copy.
- **`inplace`** (fork `Js::is_ordinary_object`): a Keep or End flush compares a tracked value with the stack's under
  the borrow while it finds only ordinary objects and data properties, and clones only when it meets anything else.
- **`hnd`** (fork `Scope::this_host_data` and `arg_host_data` with no handle; `drain_released` keeps its buffer): a host
  method's receiver and a decimal's operand are read with no lift.
- **`loc`**: a localized entity field the facade's memo holds is read without the lend.

µs a call (allocations), each row at its better of two runs. The first five columns are separate binaries; the last
three are the binary with every change, with that change's switch off:

| Row | Round 33 | `cap` | `key` | all | `inplace` off | `hnd` off | all bindings' off |
|---|---:|---:|---:|---:|---:|---:|---:|
| walk itemList, 100 context maps | 188 (3620) | 177 | 157 (2418) | 127 (1711) | 148 (2412) | 127 | 148 |
| 10 noop service calls, `itemList` tracked | 385 (6824) | 375 | 340 (5020) | 302 (4256) | 329 | 302 | 328 |
| 100 decimal multiply + add | 94 (1216) | 86 | 93 | 71 (613) | 71 | 78 (814) | 78 |
| 100 `ec.user.nowTimestamp` | 42 (702) | 39 | 42 | 31 (502) | 32 | 36 (602) | 37 |
| 100 cache gets | 34 (610) | 35 | 35 | 28 (408) | 28 | 33 (509) | 33 |
| walk an entity list, 100 cached rows | 188 (2654) | 182 | 188 | 159 (2546) | 159 | 159 | 182 |
| 1000 string concatenations | 141 | 95 | 141 | 95 | 95 | 95 | 95 |
| JSON stringify + parse of 100 objects | 110 | 95 | 109 | 95 | 95 | 95 | 95 |
| trusted template | 93 (956) | 85 | 86 (714) | 74 (569) | 78 (710) | 74 | 78 |
| Single, html | 471 (4950) | 441 | 435 (3878) | 398 (3460) | 408 | 401 | 410 |
| Single.qvt | 806 (8597) | 754 | 727 (6424) | 661 (5680) | 683 | 666 | 685 |
| List, html, 100 rows | 10724 (107828) | 9672 | 10544 | 9502 (94247) | 9505 | 9601 | 9606 |
| List.qvt, 100 rows | 16210 (131253) | 14522 | 16034 | 14487 (117626) | 14495 | 14630 | 14623 |
| List, text, 100 rows | 3046 (40959) | 2954 | 2895 | 2753 (34068) | 2764 | 2793 | 2794 |

`loc` had no column of its own: it is the entity-list walk's 159 → 182 µs in "all bindings' off", which the other two
switches do not move.

Against Round 32's oxide (and QuickJS): walk itemList 196 → 127 (59), nowTimestamp 68 → 31 (30), decimals 96 → 71
(74), Single.qvt 820 → 661 (510), List.qvt 100 16392 → 14487 (12953).

**Checks.** `cap` and `key` passed the fork's Sweep (V8 v7 instructions ≤ ×1.0000, `run_at` 15251) and `gates.sh`;
`is_ordinary_object` passed `gates.sh`. `handle-free-reads` and the merged `main` passed the fork's `embedding_api` and
`host_exotic` tests. On mr-moqui, moqui-core's `script::`, `screen::` and entity tests and moqui-entity's localize
tests passed. From here a round is proved locally like this, with no Sweep, no Mantle run and no full-suite run.

Next, from the backlog: a Release flush without the copy, strings in one copy, plain objects built in one step,
`PropertyKey::as_index`, and a bare name in one hook.

## Round 35: plain objects in one step, strings in one copy, a Release flush with no copy

The first iteration of the long run, proved locally (`js_speed`'s switch variants and the touched unit tests). The
fork's `perf/plain-objects` merged to its `main` (1251adb9), with additive API:
- `Js::new_object_from` makes a plain object's data properties by `CreateDataProperty` in one crossing; with
  `InitValue::Text` a property's Rust text becomes a heap string in one copy. `new_array_from_inits` does the same for
  an array.
- `Js::plain_own_data` and `dense_array_values` read a plain object's own data properties, or an Array's elements, in
  one crossing, running no code. They return `None` for anything else.
- `PropertyKey::as_index` reads an index key as a number, and `Scope::arg_text` reads a text argument to a `String`.

The bindings' changes, each behind a switch for one build:
- **`rel`**: a Release flush notes the stack's value by a 64-bit fingerprint, not a copy, and compares in place too.
  The fingerprint ignores a map's order, as `java_equals` does; two different values share one about once in 2^64.
- **`obj`**: maps are made with `new_object_from`, and `to_host` and the in-place flush read with `plain_own_data` and
  `dense_array_values`.
- **`idx`**: an entity list's `list[i]` uses `as_index`.
- **`str`**: map and list text is passed as `InitValue::Text`, and text arguments are read with `arg_text`.

µs a call (allocations), each row at its better of two runs. The first three columns are Round 34 and this round with
every change; the last four have that one change off:

| Row | Round 34 | Round 35 | `rel` off | `obj` off | `idx` off | `str` off |
|---|---:|---:|---:|---:|---:|---:|
| walk itemList, 100 context maps | 127 (1711) | 88 (1307) | 92 | 122 | 93 | 95 (1712) |
| 10 noop service calls, `itemList` tracked | 302 (4256) | 227 (3032) | 273 (4257) | 275 | 232 | 238 (3457) |
| 100 cache gets (a text argument) | 28 (408) | 26 (206) | 28 | 28 | 29 | 29 (408) |
| walk an entity list, 100 cached rows | 159 (2546) | 156 (2344) | 156 | 156 | 159 (2546) | 155 |
| trusted template | 74 (569) | 66 (487) | 67 | 73 | 67 | 68 (570) |
| Labels, html | 278 (2614) | 258 (2195) | 275 (2604) | 269 | 258 | 256 |
| Single, html | 400 (3460) | 347 (2962) | 359 | 395 | 352 | 352 (3152) |
| Single.qvt | 661 (5680) | 564 (4739) | 583 | 655 | 571 | 574 (5102) |
| List, html, 100 rows | 9502 (94247) | 9062 (88958) | 9183 | 9369 | 9182 | 9112 (92787) |
| List.qvt, 100 rows | 14487 (117626) | 13964 (112438) | 14097 | 14255 | 14073 | 14128 (116468) |
| List, text, 100 rows | 2753 (34068) | 2614 (32918) | 2623 | 2777 | 2636 | 2610 |

(The `rel`, `obj` and `idx` columns come from the build before `str`, compared with its own all-on row.)

Against Round 32's QuickJS:
- List html 100 (9062 vs 9268), List text (2614 vs 2876) and nowTimestamp (31 vs 30) are at or below it.
- Single html and Single.qvt are ×1.10, List.qvt ×1.08, Labels ×1.15 and the trusted template ×1.11.
- walk itemList is ×1.48, cached finds ×1.22 and the locked-down template ×1.20.

The qvt rows' profile is mostly the engine core (`run_at` 11% of Single.qvt and 24% of List.qvt, self time), so the
binding layer is no longer what they mostly spend.

## Round 36: keys found by identity, names compared as bytes, special keys by a scan

The second iteration of the long run. The fork's `perf/key-atoms` merged to its `main` (abcddb2f):
- `View::key` finds a key whose string was interned before in a direct-mapped cache, by the string's address. The
  entry holds the string, so the address is not reused while it is cached, and the cache empties when the collector
  runs.
- `JsString::eq_str` compares flat Latin-1 text with ASCII Rust text as bytes.

The bindings:
- Every map converted or compared looked up `__wrap`, `__set__` and `__proto__`, each by a SipHash lookup in the
  `IndexMap`, and so did `converts_with_script` for every map a bare name reads. `has_special_key` scans the keys'
  first bytes first.
- `text_is` uses `eq_str`.

Measured all on against all off, from one run each, because the run was cut short (µs a call):
- walk itemList: 88 → 78 (`spk` off 84, `eqs` off 79);
- 10 service calls with `itemList` tracked: 228 → 214;
- the trusted template: 68 → 65;
- Labels: 271 → 255;
- Single html: 368 → 346;
- Single.qvt: 591 → 560;
- List text: 2656 → 2601.

`katom` has no number of its own. From here, each change's own effect is left to the final acceptance, and an
iteration times only all on.

The profile also found that each lend of the engine to a non-leaf host call swaps the 4.2 KB `Heap` and the
`Services` by value, out and back: about 10% of the cached-find row (`lend`, `NativeCx::restore`). A pointer swap
would need the interpreter to reach the heap through a box, so it is noted, not done.

## Round 37: the allocator (measured; a proposal, nothing changed)

malloc and free are 10 to 15% of every oxide row's profile, and the server uses glibc's malloc (no
`#[global_allocator]`). The Round 36 binary was run with glibc and with jemalloc (`LD_PRELOAD` of the system's
`libjemalloc.so.2`), two runs of each, on both engines (µs a call, better run):

| Row | oxide glibc | oxide jemalloc | QuickJS glibc | QuickJS jemalloc |
|---|---:|---:|---:|---:|
| walk itemList | 77 | 68 | 61 | 61 |
| 10 cached entity finds | 52 | 49 | 45 | 43 |
| 10 noop service calls | 92 | 82 | 86 | 77 |
| 1000 string concatenations | 94 | 73 | 139 | 115 |
| locked-down template | 80 | 73 | 67 | 64 |
| Labels, html | 252 | 237 | 228 | 224 |
| Single, html | 345 | 318 | 320 | 314 |
| Single.qvt | 559 | 509 | 522 | 512 |
| List, html, 100 rows | 9109 | 8054 | 9593 | 9256 |
| List.qvt, 100 rows | 14063 | 12606 | 13282 | 13040 |
| List, text, 100 rows | 2639 | 2264 | 2958 | 2743 |

jemalloc takes 5 to 15% off oxide's rows and 1 to 10% off QuickJS's. QuickJS gains less because its own heap uses
the C library's `malloc` less than oxide's arenas and the bindings do. With jemalloc, oxide is at or below QuickJS on
every screen row but Labels (×1.06), and ×1.11 on walk itemList. The same runs give today's QuickJS on glibc: the
Round 32 numbers above came from a quieter machine.

Adopted: `crates/moqui/src/main.rs` and the binary `./moq build` generates set `tikv_jemallocator::Jemalloc` as the
global allocator (not on MSVC, which keeps the system's). To go back, remove those three lines and the dependency.
The tests keep their own allocators (`moqui-host`'s counting one), as the allocator is set only in the binaries.

The server's memory, started on this tree, idle 20 seconds, then 50 page loads:
- glibc: 480 MB at its peak and at the end;
- jemalloc: 603 MB at its peak (startup's data load and search indexing), then 502 MB.

jemalloc holds freed pages until its decay returns them. `_RJEM_MALLOC_CONF` (for example `dirty_decay_ms`) tunes that
if the peak matters.

## Round 38: a plain object's strings compared and copied where they lie

The fork's `perf/visit-plain-data` merged to its `main`. `Js::visit_plain_own_data` shows a plain object's own data
properties to a visitor in one crossing:
- a string as its code units, borrowed from the heap (`TextRef`, with `eq_str` and `to_string_lossy`);
- a callable as a marker, with no handle;
- anything else lifted as before.

`plain_own_data` had lifted every string into a `JsString` (two allocations and a copy) only to compare it or copy it
again. In the bindings:
- the flush's in-place comparison of a map compares its text with the host's text where it lies, and walks the other
  values after;
- `to_host` copies text once, into the host `String`.

A text value whose host value is not a string (a time stamp shown as text) takes the comparison as before.

Against Round 36's binary, two runs each (µs a call, allocations):
- walk itemList: 77 (1307) → 70 (907);
- 10 service calls with `itemList` tracked: 213 (3032) → 198 (2232);
- the trusted template: 64 → 62;
- Labels: 255 → 246;
- Single html: 346 → 339;
- Single.qvt: 561 (4739) → 545 (4104);
- List.qvt 100: 14186 → 13963;
- List text: 2666 → 2591.

## Round 39: a tree of plain maps and lists made in one crossing; text values read without a lift

The fork's `perf/init-trees` merged to its `main` (dd3624b9). `InitValue::Object` and `InitValue::Array` make a nested plain
object or array inside the outermost `new_object_from` or `new_array_from_inits`, in its one view, with no handle of
its own.

The bindings:
- A host list or map nested in another, with no special key, is passed as such an init, so a list of 100 maps is one
  crossing and one handle.
- A `Value` or `OptValue` argument that is a string is read with `Scope::arg_text`, not lifted to a `JsString` and
  copied again.

Allocations a call against Round 38's binary:
- List.qvt 100: 111676 → 103774;
- List html 100: 87996 → 80154;
- List text: 32156 → 29310;
- Single.qvt: 4104 → 4004;
- walk itemList: 907 → 807.

Times were within the noise. Another checkout's `./moq test` ran during the timing, and three reruns of the same binary
spread 5 percent. This round's times are left to the final acceptance, on a quiet machine.

## Round 40: the regexp executor's buffers kept between searches

A List.qvt render's allocations were traced with heaptrack, its backtraces summed by their first source line. The top
sites:
- the engine's short-string concatenation (`vm/ops.rs`): 21%, one buffer a `+`, as QuickJS's `malloc` makes;
- the API's handle slots: 9%;
- the `JsString`s of lifted text: about 18%, which Rounds 38 and 39 cut;
- the regexp executor: about 11%. Each search made its captures, registers, undo log and backtrack stack anew, and grew
  the last two a push at a time.

The fork's `perf/regexp-scratch` merged to its `main` (2742f610). A search's buffers go back to a thread slot when its state
drops, and the next search clears and reuses them. Ones a deep backtrack grew past 4096 entries are let go.

Against Round 39's binary, on a quiet machine (µs a call, allocations):
- List.qvt 100: 13790 (103772) → 13523 (91939);
- List html 100: 8914 (80154) → 8816 (74857);
- List html 20: 1921 → 1899;
- Single.qvt: 549 (4004) → 546 (3766).

The other rows are unchanged.

## Round 41: a host call's receiver read with no handle

Of a List.qvt render's handle slots, most came from one line. A host method whose receiver held no `OxideHost` datum
(an `ec` object's method, or an entity value's or list's) lifted the receiver into a handle, only to ask
`receiver_state` whether it was an entity value or a list. `receiver_state` now reads the call's receiver with
`Scope::this_host_data`, and the receiver is lifted only when a method returns it (`Ret::This`).

Against Round 40's binary (µs a call, allocations; another job ran during it, so the small rows are noise):
- List html 100: 8883 (74857) → 8535 (68432);
- List html 20: 1920 → 1844;
- List.qvt 100: 13520 (91938) → 13178 (85395);
- List text: 2496 (29310) → 2409 (27190);
- Single.qvt: 545 → 538;
- Labels: 246 → 240.

## Round 42: an element literal's held conversion read with no handle

A template's element literal holds its conversion, a host object, under `$nodeLit`. `lit_of` had lifted that value
into a handle to read its datum, and `convert_args` had lifted the argument first. The fork's
`perf/property-host-data` merged to its `main` (2469ea98):
- `Js::property_host_data` reads the host datum of `object[key]` with no handle for the value;
- `Scope::arg_property_host_data` does the same for a call's argument, with no handle for the argument either.

Node arguments use the latter, and `lit_of` the former.

Against Round 41's binary, on a quiet machine (µs a call, allocations):
- List text 100: 2435 (27190) → 2375 (26190);
- List html 100: 8609 (68432) → 8535 (67030);
- List.qvt 100: 13237 (85394) → 13134 (83392);
- Single.qvt: 541 (3672) → 536 (3614).

## Round 43: a RegExp replace joined as bytes (no row moved)

The locked-down template, the row furthest behind QuickJS (81 against 67 µs), showed `String.prototype.replace` with
a RegExp in its profile, and building and narrowing UTF-16 buffers (`Vec<u16>` collection and
`StringRepr::from_units`).

The fork's `perf/replace-one-byte` merged to its `main` (7cf1f530). `replace_pristine` had copied the whole subject to UTF-16,
built the result in UTF-16 and narrowed it at the end. It now joins the result as bytes until a wide unit arrives,
copying the subject where it lies, and makes the subject's UTF-16 copy only for a `$` substitution.

Checks:
- The outputs match `main`'s over 1051 cases: Latin-1 and two-byte subjects; literal, `$` and functional
  replacements; empty, sticky and non-global matches.
- An escape-style loop in `qjs` runs in 422 ms against 496.

No `js_speed` row moved, the locked-down template included, so its gap is the interpreter on the template's own code.
The sandbox's setup was checked too: default heap options, and a deadline that only reads the clock.

## Round 44: a fragmented runtime starts fresh

The heap, measured on mr-mantle's `OrderToCashBasicFlow` under `oxide` with `MOQUI_SCRIPT_HEAP_LOG`:
- 58 runtimes started, at most 25 live, 45 MB reserved across them at the peak;
- the main thread's runtime ended at 15.0 MB reserved for 5.5 MB live.

That 15.0 MB was:
- tables: 3.8 MB;
- dynamic code: 3.0 MB;
- strings: 2.6 MB;
- values: 2.3 MB, a quarter of the slots used;
- objects: 1.5 MB, for 73 KB live;
- internals: 0.6 MB, for 39 KB live.

**Chunked arenas, measured and dropped.** A temporary probe counted the 1024-slot chunks that hold a live record:
- objects: 40 of 46;
- strings: 35 of 36;
- functions: 11 of 14;
- internals: 3 of 5.

The survivors of a peak are scattered, so freeing empty chunks would give back 10 to 15% of the arenas. It would also
cost an indirection on every record access in the interpreter.

**The runtime instead.** At an outermost run's end, a runtime that holds at least 8 MiB reserved and 2.5 times its last
collection's live bytes is dropped, and starts fresh at the next script (`FRAGMENTED_BYTES`). A collection confirms it
first, and a thread does this at most every 30 seconds. The stats count it as `shed_fragmented`.

On the same spec:
- the main runtime ends at 6.2 MB reserved instead of 15.0, and its peak is 11.0 MB instead of 17.1;
- the process's peak across live runtimes is 42.6 MB instead of 45.2: the other 25 runtimes are each under the 8 MiB
  floor, at 1 to 1.8 MB each, mostly a fresh realm's builtins and caches;
- all 42 features pass, in the same time.

`js_speed` dropped no runtime (the same allocations on every row), and its times were within the noise of a busy
machine. What is left of a process's script heap is that per-runtime baseline across many threads.

## Round 45: a runtime's start slack (measured, dropped); the fragmented check confirmed by a collection

A fresh runtime's reserved bytes carry the growth slack of the bindings' start: the shims, helpers and freeze grow
the arenas again after the core gave its realm's slack back. A fork branch (`feat/shrink-to-fit`, unmerged) added
`Runtime::shrink_to_fit`, and the bindings called it after a runtime's start.

On `OrderToCashBasicFlow` it did not pay. The runtimes grow the slack back as soon as they run scripts:
- the spec runtime ended at 1.60 MB against 1.69;
- the process peak was 45.8 MB against 42.6, within the noise of when the concurrent runtimes peak;
- only the main runtime's first spec ended smaller, at 0.8 MB against 2.9.

The branch was dropped.

It showed a flaw in Round 44's check. Its first look compared the reserved bytes with the last collection's live
bytes, which may be a burst's, still live then, and so too high. The check now collects to confirm, for any runtime
past 8 MiB, at most every 5 seconds a thread, and still drops at most every 30 seconds.

## Round 46: inline small strings (measured, not built)

Short string concatenation (`vm/ops.rs`, a buffer a `+`) is 28% of a List.qvt render's allocations. A string record is
held to 40 bytes, and its `OneByte` box leaves room for 7 bytes inline. A probe counted List.qvt's short one-byte
concatenations by the result's length (11.2 million):

| Length | Share | Cumulative |
|---|---:|---:|
| up to 7 | 14% | 14% |
| 8 to 14 | 32% | 46% |
| 15 to 22 | 16% | 62% |
| 23 to 30 | 15% | 77% |
| 31 to 63 | 23% | 100% |

malloc and free are about 14% of List.qvt's time on glibc, so all the concatenations' allocations are worth at most
about 4% of the row there, and less under jemalloc, the server's allocator now. Seven inline bytes would take under
1%. Fourteen would take about 2% and make every string record 8 bytes larger. It was not built.

## Behaviour changes to know

- In a transaction-cache frame (`transaction="cache"`, `force-cache`), a row the frame read is not read again: another transaction's later commit of it is not seen until the frame ends (plans/TX-Cache.md D2). Held writes go to the database at the commit, merged per row; a failure there names the write and the service that held it. `heartbeat#UserStatus`, `mark#Read`, `send#Message`, `update#WikiPage` and `set#MessageFlags` hold their writes now.
- A chat message is indexed for search after its transaction commits, on a worker, not in it.
- On Postgres a `SAVEPOINT` with nothing unanswered before it, and every `RELEASE SAVEPOINT`, go out with the next request; a failure of either is reported there. A `COMMIT` goes out behind the writes still unanswered unless one has a check made here; a write that fails is then the commit's error, and the server has already ended the transaction (the rollback after it is a no-op).
- `ec.resource.getLocationReference` reads a reference's size, last-modified time and children when a script first asks for them, not when it is made; a `dbresource` directory reference lists its children then too. An entry of `getDirectoryEntries` is built from the listing and reads its size when asked.
- The count of awake `EmailUserState` rows that every message write reads is cached; `EmailUserState` is `cache="false"` (it was `never`).
- A transaction-cache frame's commit batch is answered by the commit's wait: a failure of a held write is the commit's error ("commit failed", with the database's message), not "Could not write the transaction cache's N held writes". A held update of a row the frame read with a locking find is not checked for a changed row.
- A Postgres `COMMIT` the server answers with `ROLLBACK` (a statement of the transaction failed and its failure was reported before) is an error.
- A service call builder of a script hands its parameters to the call: calling the same builder again sends only what was set since.
- On Postgres a transaction-cache update the frame did not lock is sent as a `WITH ... UPDATE ... RETURNING` statement the server fails when no row changed; its error is still "the record was not found".
- A Postgres statement first sent deferred on a connection is prepared with text parameters typed by the server, not described first.
- An async service call or job logs in as its user through the entity cache.
- A select-field find in a transaction-cache frame is answered from what the frame read until the frame writes that entity.
- `send#WebPush` runs with no transaction around its pushes and writes their outcomes after them, in one batch.
- `ResourceLock` is `cache="false"`; a write's lock check reads a cached count of all locks first. `save#Draft` and `process#InboundItem` are `transaction="cache"`.
- `TraceContext.trace_id` and `span_id` are `HexId` values (their bytes), not `String`s; `to_string()` gives the hex.
- A row created in a transaction-cache frame reads back with every field (null where unset), as the database gives it.
- `expire#UserStatuses` writes its users in batches (`publish#UserStatusBatch`), split down to a failing user.
- 45 application and framework services are `transaction="cache"` (Round 23's list), among them `place#Order`, `approve#Order`, `change#InvoiceState`, the Shop checkout steps and `CallServices.start#Call`: their writes go out at the commit and a row they read is not read again in the transaction.
- A time stamp held in a transaction-cache frame reads back in the frame at the precision the database keeps (Postgres microseconds).
- On Postgres a view read in a transaction-cache frame no longer writes the held rows first: it reads the member tables with the held rows in place of their stored ones (a sub-select of each), so the held writes still go out at the commit.
- Mantle's asset reservation, issuance, receipt and physical inventory services, and `cancel#OrderItem` and `reject#OrderItem`, are no longer `no-tx-cache`.
- A `distinct` or `having` find or count in a transaction-cache frame of an entity it holds rows of reads them in place (Postgres) instead of writing them first; so does a view read of a member with an encrypted field.
- `create#Payment`, `apply#Settlement`, `pack#Shipment` and `ship#OrderPart` are `transaction="cache"`: `create#Payment` called in another service's transaction turns holding on for the rest of it.
- A view built in code (a data document, a REST query view) read in a transaction-cache frame reads the held rows in place (Postgres) instead of writing them first.
- `entity_tx_cache` (default true) turns the transaction cache off everywhere when false.
- 13 more Mantle services are `transaction="cache"` (Round 26's list); `complete#ShipmentPack` calls `pack#Shipment`, and Customer 360 its service, with `transaction="force-cache"`.
- A script engine's QuickJS heap is allocated through Rust's allocator (rquickjs `RustAllocator`, counted), not QuickJS's malloc functions; compiled script bytecode is shared by the process's engines.
- `screen_tx_cache` (default false): true makes screen transitions' and `begin-transaction` screens' transactions hold, as `cache`.
- A script that runs while another runs on the thread runs in the same script engine and context, not in an engine of its own; its globals are the thread's. A screen mode's outermost unit gives back the render state around it.
- A module `ec.resource.script` loads whose top level only declares is the same value for the rest of the outermost script run (`script_module_cache`, default true).
- The test harness's spec code runs in a script engine of its own.
- Screen transitions' transactions (and `begin-transaction` screens') hold their writes in the transaction cache by default (`screen_tx_cache=false` to begin them as `use`).
- `moqui test --db snapshot` keys a snapshot by the day too, and removes snapshots unused for two days.
- SMTP-delivered Inbox mail and its account rules commit together when no email ECA is installed; a failure there stores the message alone and runs the rules after.
- Mail stored by IMAP and POP3 sync, SMTP delivery and import, and the IMAP flag sync, run in transaction-cache frames; `react#Message`, `edit#Message`, `redact#Message`, `vote#Poll`, `mark#AllRead` and `mark#NotificationsViewed` are `transaction="cache"`, and a redacted message leaves the search index after its commit.
- `no-tx-cache` turns a frame to read-through (writes go to the database, reads still kept), and a later `cache` call that joins it no longer turns holding back on.
- A find of a view built in code (a data document, a REST query view) writes the frame's held rows first, as a view entity read does.

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
10. **A Java comparison.** Run the same `Tools/Database/SpeedTest.xml` calls and a seed load on Moqui Java against the same Postgres, so the numbers above have a reference.

Known flaky test, not from this work: `service::coverage::tests::calls_are_counted_only_between_enable_and_finish` counts into a process-wide recorder and can race with tests running in parallel.
