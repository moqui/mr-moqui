# Profiling

Status 2026-10-21: four rounds of profiling and optimization of entity, service, data load and XML/definition work against embedded Postgres, built and committed. Round 5 tested and measured against an external Postgres 18 in Docker ([Round 5](#round-5-external-postgres)), fixed what it found, and sends a data load's rows several to a statement ([Round 6](#round-6-multi-row-data-load-statements)). Round 7, on an M1 Pro (macOS), measured COPY and dropped it, added a multi-threaded benchmark and fixed the lock it showed, and cut what a transaction keeps per write ([Round 7](#round-7-macos-threads-copy-bookkeeping)); what is left leads the [Next steps](#next-steps). Companion to [Performance-Summary](Performance-Summary.md), which digests the performance plan; this document records what was measured and changed, how to measure, and what is left.

## How to measure

Five opt-in speed tests in `crates/moqui-host/tests/` (each `test = false` and `#[ignore]`, so a default `cargo test` neither links nor runs them):

| Test | What it times | Needs |
|---|---|---|
| `entity_speed` | entity facade finds (cache, PK, list, count, iterate 100 rows) and writes (create, update, delete, `create_or_update`), in one transaction and in autocommit; then a cached find, a PK find and writes on 1, 2, 4 and 8 threads at once (`ENTITY_SPEED_THREADS`; `ENTITY_SPEED_THREADS_ONLY` runs that section alone) | embedded Postgres |
| `service_speed` | the `SpeedTest.xml` calls: entity-auto create/update/delete of `moqui.test.TestEntity` and `echo#Data`, plus the same entity calls direct, in one transaction and each in its own | embedded Postgres |
| `data_load_speed` | XML create and reload, CSV, JSON, nested records, and the framework seed data in `moqui load` order (first load and reload) | embedded Postgres |
| `xml_speed` | `MNode` parse, keep, read, lookup and drop over the 784 runtime XML files, and loader-style record elements | nothing |
| `definition_speed` | the framework entity model, the service definitions, and the base components' screen definitions | nothing |

Run one with `CARGO_PROFILE_RELEASE_DEBUG=1 cargo test --release -p moqui-host --test <name> -- --ignored --nocapture`; each file's header names its size variables. `SPEED_PG_HOST` (with `SPEED_PG_PORT`, `SPEED_PG_USER`, `SPEED_PG_PASSWORD`, `SPEED_PG_SSLMODE`, `SPEED_PG_SSLROOTCERT`) runs the three database tests on a new database of an external server instead, dropped at the end (`speed::SpeedDb`; the role needs `CREATEDB`). The shared harness (`tests/speed/mod.rs`) prints, per call: median time, allocations, bytes allocated, bytes kept after the batch (a cache or a leak), and SQL statements (`entity::statement_stats`, a round-trip count). The allocation columns come from a thread-local counting allocator and do not change from run to run, so they are the steady measure on a busy machine.

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
- **Postgres wire client** (`crates/moqui-host/src/db/pgwire.rs`): the protocol over a blocking Unix or TCP socket, TLS by rustls, `postgres-protocol` for message encoding, parsing and SCRAM. `tokio-postgres` and `tokio-postgres-rustls` are removed. A round trip is one write and one read. Every request ends with `Sync`, so an unanswered one (BEGIN, `SET LOCAL`, a deferred write, a statement close) is read and checked by the next call that waits.
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

**Functional.** `./moq load` (8,740 records in 4.9 s), the server (REST reads and writes through mantle, tools and system screens) and `./moq test mantle-usl --db existing` (557 features, 61 specs, all passed in 74 s) work against `moqui-pg18`.

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
5. **Remaining copying MNode calls in definition builders** (comparisons done in round 7, 1.5% of the entity model's allocations). `name()`, `attribute()` and `children()` still copy; screen, form and service builders use them a few times per node. The borrowing calls exist; moving the busiest callers (screen `form.rs`, `definition.rs`, `service/define.rs`, the moqui-db model) would cut definition load further. The entity model's remaining allocations are mostly its own `String` fields.
6. **Nested data records** still go through an `MNode` per top-level element; a streaming builder could read them the way `parse_flat` reads flat ones.
7. **Reads before write** for entities that cannot upsert (audit-logged, status flows, data-feed entities) still cost one waited round trip each in a load.
8. **QuickJS for trivial actions.** About half of `echo#Data` is QuickJS running four `<set>` actions through its proxy; a native path for trivial actions would change value types, so it needs a decision.
9. ~~Transaction bookkeeping~~: done (round 7) for entities with no cached find and no feed document. A data-feed touch still keeps two `String`s per write; an entity with cached finds keeps the values written.
10. **A Java comparison.** Run the same `SpeedTest.xml` calls and a seed load on Moqui Java against the same Postgres, so the numbers above have a reference.

Known flaky test, not from this work: `service::coverage::tests::calls_are_counted_only_between_enable_and_finish` counts into a process-wide recorder and can race with tests running in parallel.
