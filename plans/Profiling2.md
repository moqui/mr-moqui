# Performance: where it stands, what remains, and the ideas

Status 2026-11-08. This file replaces four running logs, now in `archive/`:
- [Profiling](archive/Profiling.md): Rounds 1 to 49.
- [TX-Cache](archive/TX-Cache.md): the transaction cache, Phases 0 to 6 and Censuses 1 to 9.
- [Script-Engine-Oxide](archive/Script-Engine-Oxide.md): quickjs-oxide in and rquickjs out, Phases 0 to 4.
- [Script-Engine-Zipp-Ideas](archive/Script-Engine-Zipp-Ideas.md): ideas from zipp.org.

It keeps the latest numbers, what is still open and the ideas not yet built. Round 50 (2026-11-08, [below](#round-50-tiers-0-to-2))
built §5's tiers 0 to 2. Each one cites where it was measured or proposed: "R*n*" is a Profiling round, "TX §*name*" or "TX D*n*" a section or decision of TX-Cache, "Oxide §*n*" a
section of Script-Engine-Oxide, and "Zipp *n*" an item of Script-Engine-Zipp-Ideas §3.

Most of the database and service work is done:
- The entity, service and data-load paths have been profiled and optimized against embedded and external Postgres.
- The transaction cache holds writes and serves reads for 60-odd framework and application services. It took the
  waited round trips of the 45 services it was first applied to down 36% (R25), and the Mantle operations 35% (R27).
- The script engine is quickjs-oxide alone (Oxide §7, Phase 4 result). Its screen rows are within ×1.10 of QuickJS's
  last numbers, and plain JavaScript is faster than it was (R48).

What is left is mostly two things. Two script rows still lose to QuickJS at the boundary between host and engine (walk
itemList ×1.15, cached finds ×1.04, R50). And the heap of the script runtimes is still over its gate in some test
processes (R49, R50).

## 1. Headline numbers

| Area | Latest | Proof |
|---|---|---|
| Entity | PK find 19.7 µs (28 allocations), cache hit 0.82 µs, list of 100 rows 232 µs; create, update and delete in one transaction 27, 30 and 22 µs | R4 Results |
| Service | create in its own transaction 60 µs; update and delete 33 and 24 µs; `echo#Data` 8.9 µs | R4, R11 |
| Threads | a cached PK find at 1.06M calls/s on one thread, 956k on eight (M1 Pro) | R7 |
| Data load | 10.2 µs a record embedded, 11.0 µs with 200 µs to the database; CSV 9.0 µs; a seed reload 18.7 µs; `./moq load` of 8,740 records 4.34 s | R6 |
| External Postgres | a statement waits 1 round trip, a service in its own transaction 2; iterating 100 rows waits 3 (1,087 µs at 200 µs) | R5 |
| Transaction cache | waited round trips of the 45 services 324,548 → 207,658 (−36%); 42 early flushes in Census 9, all past the size caps | R25, R26 |
| Mantle operations (200 µs to the database) | 6,037 → 3,954 ms with the cache; oxide 3400 ms against QuickJS's 3380 (+0.6%) | R27, R48 |
| Script engine | js_speed's 59 rows ×0.90 of R49 (geometric mean); screen rows ×0.89 to ×0.97 of R49; the locked template ×0.75, fancy ×0.73; cached finds ×0.90; numbers to text and one-character strings ×0.40, the regexp scan ×0.58 | R50 |
| Runtime start | 1.57 ms, 9,277 allocations, 2.9 MB allocated (8.96 ms, 47,436, 12.3 MB before) | R50 |
| Script heap | test-apps process peaks 15 to 86 MB, three of six under the gate of 73 MB (R49: 16 to 83 MB, also three) | R50 |
| Startup | first start 5.63 s (the empty-database load about 1.15 s), restart 268 ms | R9 |
| Definitions and XML | entity model 5.3 ms, service definitions 8.7 ms, 112 µs a screen definition; MNode parse 76 µs a file | R4, R8 |

## 2. How to measure

**The speed tests** are in `crates/moqui-host/tests/`. Each is `test = false` and `#[ignore]`, so a plain `cargo test`
neither links nor runs them. Run one with:

```
CARGO_PROFILE_RELEASE_DEBUG=1 cargo test --release -p moqui-host --test <name> -- --ignored --nocapture
```

| Test | What | Switches |
|---|---|---|
| `entity_speed` | finds, writes, 1, 2, 4 and 8 threads | `ENTITY_SPEED_THREADS`, `ENTITY_SPEED_THREADS_ONLY` |
| `service_speed` | `Tools/Database/SpeedTest.xml`'s calls; `echo#Data` is the held script call | |
| `data_load_speed` | XML, CSV, JSON, nested records, the seed data | |
| `xml_speed`, `definition_speed` | MNode over the 784 runtime XML files; entity, service and screen definitions (no database) | |
| `js_speed` | expressions, scripts, actions, services, templates, screens, a new thread's first script | `JS_SPEED_ONLY`, `JS_SPEED_ROW`, `JS_SPEED_CALLS`, `JS_SPEED_BATCHES`, `JS_SPEED_DUMP`, `JS_SPEED_PEEK` (`--features embedded-postgres`) |
| `startup_speed` | a start and a restart | `STARTUP_SPEED_COMPONENTS`, `STARTUP_SPEED_LOAD`, `STARTUP_SPEED_LOG` |
| `tx_cache_speed` | rows S1 to S5, F1 and F3 to F5, N1 to N4, cache off against on; an `rt` column of waited round trips | `TX_CACHE_SPEED_ROW` |

- **The harness's columns** (`tests/speed/mod.rs`): the median, allocations, bytes, bytes kept, SQL statements and
  waited round trips. Allocation counts are exact from run to run, so they are the steady measure on a busy machine.
  `js kept` is the bytes the thread's script runtime reserves at the end of a batch less its start.
- **External Postgres:**
  - `SPEED_PG_HOST` (with `SPEED_PG_PORT`, `_USER`, `_PASSWORD`, `_SSLMODE` and `_SSLROOTCERT`) makes a database there
    and drops it at the end. The role needs `CREATEDB`.
  - For a delay, run `tc qdisc add dev eth0 root netem delay 200us` in a container started with `--cap-add NET_ADMIN`,
    and connect to its bridge address, not a published port (R5).
  - `tests/it/pg_external.rs` (`PG_EXTERNAL_URL`) checks timeouts, cancel and TLS.
- **Tools:**
  - samply: `samply record --save-only --unstable-presymbolicate`, with `perf_event_paranoid` at 1 or less.
  - heaptrack: `heaptrack_print X.zst --flamegraph-cost-type allocations -F stacks.txt`.
  - instruction counts: `perf stat -e instructions:u` with one batch and with six.
  - a frame-pointer profile: `RUSTFLAGS="-C force-frame-pointers=yes" CARGO_TARGET_DIR=target/fp`, then
    `perf record -F 999 -g` on one row (R15).
  - DWARF unwinding (`--call-graph dwarf`) did not resolve the script runtime's frames on a new thread (R49).
  - Also `strace -ff`, the server's `log_statement = all` (R16), and macOS `sample <pid>` for lock waits.
- **The transaction cache:**
  - `MOQUI_TX_CACHE_STRICT=true` makes an early flush in a holding frame an error (TX D1).
  - `MOQUI_TX_CACHE_EVERYWHERE=true` makes every frame hold, for a census run.
  - `MOQUI_TX_CACHE_CENSUS_FILE` writes a line per transaction, with `micros` since R26.
  - The Mantle operations are timed with `-Dentity_tx_cache=false` and `--db existing` against a template database
    (R26).
- **The script engine:**
  - `MOQUI_SCRIPT_STATS_FILE=<file>` appends a JSON line per process: scripts by kind, runtimes started, live and
    restarted, the heap at its peak, and `heap_at_max_by_runtime`, each runtime's share of the peak by thread (R49).
  - `MOQUI_SCRIPT_HEAP_LOG=<file>` writes a line per spec with the main thread's runtimes' arenas, code and caches.
  - A round's changes go behind a temporary `MOQUI_OXIDE_OFF` switch, so one release build measures each
    (`../oxide-bench/round33/run-variants.sh`, `table.py`).
- **The fork** (`../oxide-bench`, outside any repository, its `README.md`):
  - **The Sweep.** Build a base from the fork's current `main`, then `bench.py sweep` and `bench.py memory` over
    `suite-matrix/manifest.json`, in a quiet window. The gates: each of the eight V8 v7 programs at most ×1.02 of the
    base's instructions, and `run_at` under 16384 bytes.
  - **`gates.sh <worktree>`** runs upstream's CI steps, GC stress included.
  - **Merging.** A branch merges `--no-ff` into the fork's `main` with the Sweep's numbers in the message. The user
    pushes the fork, then mr-moqui's pin moves and `cargo update -p quickjs-oxide` runs.
  - No upstream issues or pull requests without asking.
  - **`mantle-pg/`** times the nine Mantle operations against `moqui-pg18` 200 µs away: `setup.sh`, `run.sh`,
    `table.py`, `teardown.sh`. It ran two engines in turn; it now runs one.
- **Rules learned:**
  - Prove locally (the rows, a profile, the touched tests), then commit. The Sweep, Mantle runs and full suites are for
    an acceptance.
  - Nothing else builds while a timing runs. Run two of each and compare them at the better one.
  - Binaries and traces go under `target/profiling`, not `/tmp` (a tmpfs filled up once).
  - Under low load, memory matters more than the last millisecond; under high load, speed does.

## 3. Where things stand

### Entity, service, data load and XML

Rounds 1 to 9 took the per-call allocations, statements and round trips down. The numbers are in the table above.
What changed is listed in [Behaviour changes](#7-behaviour-changes-to-know). Since then the rounds have been about the
transaction cache and the script engine; these paths have not been profiled again since R9.

### The Postgres driver

Statements are named and prepared per connection; `BEGIN`, `SET LOCAL statement_timeout` and the first statement share
one write; savepoints, release and `COMMIT` are pipelined. Deferred writes go out as multi-row statements (R5, R6). A
connection the server ended fails with its reason, checked by `connections_fail_cleanly_when_the_server_ends_or_cancels_them`
(R5). TLS: `sslmode=require` encrypts without verifying, `sslrootcert` makes it verify.

### The transaction cache

The cache is built (TX status line, Phases 0 to 6) and on for the framework's mail, chat, notification and wiki
services, the 45 application services of R23, 13 more in R26, and Mantle's payment and shipment services (R25). Each
transaction is an `AT_TRANSACTION` span with the cache's counts. The decisions in force, which code comments cite:
- **TX D1:** a frame holds, reads through or is off; the counts of early flushes by cause, entity and service; strict and
  census modes.
- **TX D2:** a read cache of rows by whole key, as Java's `readOneCache`; up to about 10,000 rows. A held frame does not
  see another transaction's later commit of a row it read.
- **TX D3:** list finds with no limit kept under their find key and kept current; finds pinned to a created row answered
  from the held creates.
- **TX D4:** `for-update` finds always go to the database. Select-field finds read through the overlay. `distinct`,
  `having` and aggregate finds read held rows through shadows (R25).
- **TX D5:** a view read flushes only when a held member row may match its condition; since R24 held rows are shadowed
  instead (shadows kept as arrays up to 10,000 rows, R27).
- **TX D6:** the commit batch, multi-row inserts, updates with the zero-rows check, and `COMMIT` pipelined behind it
  (R17). MySQL and SQLite still send one statement at a time.

The per-row results (TX §Results, R22's table): `update#WikiPage` 21.4 → 6.8 ms, `heartbeat#UserStatus` 2.2 → 1.2
ms, the mail store with two attachments 14.8 → 8.7 ms, all at 200 µs to the database.

### The script engine

quickjs-oxide, a QuickJS-compatible engine in safe Rust, is the only engine, pinned to the fork's `main` 1cdb77e2
(R50; 7cf1f530 before, Oxide §7, Phase 4 result).

The fork's changes:
- **F1:** compiled code shared by the process. Code images (step 1) and shared unit bodies (step 2) are done; shared
  instructions (step 3) are not.
- **F2:** exotic host objects for bare names and entity values. Done.
- **F3a:** re-entry through `Scope::lend`, at about 0.2 µs a lend. Done.
- **F3:** compile from a host function. Done; a module run from a host function is deferred, as nothing calls it.
- **F4:** speed and footprint. V8 v7 instructions ×0.974; RegExp ×0.805.
- R34 to R43 added an embedding API to the fork: interned keys, host data read with no handle, plain objects made and
  read in one step, key atoms, the regexp scratch buffers and others.

R48's rows (µs a call, QuickJS's last numbers against oxide now):

| Row | QuickJS | oxide | ÷ |
|---|---:|---:|---:|
| walk itemList, 100 context maps | 58.6 | 68.2 | 1.16 |
| 100 `ec.user.nowTimestamp` | 29.6 | 31.7 | 1.07 |
| 10 cached entity finds | 43.9 | 50.7 | 1.16 |
| 1000 string concatenations | 143.5 | 96.0 | 0.67 |
| fancy, locked down | 69.7 | 81.0 | 1.16 |
| Labels html | 226.2 | 248.7 | 1.10 |
| Single html | 317.5 | 340.1 | 1.07 |
| Single.qvt | 514.1 | 546.5 | 1.06 |
| List html, 100 rows | 9491 | 8600 | 0.91 |
| List.qvt, 100 rows | 13254 | 13316 | 1.00 |

Before Round 33, oxide was ×1.3 to ×3.3 slower at host calls (R32). The fork's Sweep of 7cf1f530 passes its gates
(V8 v7 ×0.989, `run_at` 15,251 bytes) and so does `gates.sh` (R48). R50's rows are in [Round 50](#round-50-tiers-0-to-2);
the Sweep was not run for it.

### Round 50: tiers 0 to 2

§5's tiers 0 to 2, built in three lanes, each item measured by its js_speed rows (allocations and `instructions:u`) and
its own tests. The quiet pass, the best of two runs of the R49 build (fork 7cf1f530, 16 codegen units) against R50
(`target/profiling/r50/final/table.md`):

| Row | R49 µs | R50 µs | ÷ | Allocations |
|---|---:|---:|---:|---|
| engine start (a thread's first script) | 8958 | 1568 | 0.18 | 47,436 → 9,277 |
| 1000 numbers to text | 377.9 | 149.3 | 0.40 | 5,883 → 994 |
| `split('')` + `charAt`, 1000 characters | 111.9 | 44.8 | 0.40 | 4,005 → 5 |
| regexp scan of a 200-line log | 301.1 | 175.2 | 0.58 | 2,179 → 1,578 |
| JSON stringify + parse of 100 objects | 94.6 | 69.2 | 0.73 | 1,215 → 421 |
| fancy, trusted | 65.9 | 47.8 | 0.73 | 325 → 304 |
| fancy, locked down | 81.2 | 60.8 | 0.75 | 824 → 350 |
| `JSON.parse` of the locked snapshot | 9.28 | 6.95 | 0.75 | 124 → 42 |
| `$esc` over 100 strings | 48.6 | 39.9 | 0.82 | 510 → 320 |
| List, text, 100 rows | 2432 | 2174 | 0.89 | 26,582 → 23,871 |
| 10 cached entity finds | 50.6 | 45.8 | 0.90 | 576 both |
| Single, html | 332.2 | 318.1 | 0.96 | 2,612 → 2,410 |
| List.qvt, 100 rows | 13107 | 12613 | 0.96 | 83,062 → 80,323 |
| List, html, 100 rows | 8288 | 7911 | 0.95 | 66,972 → 65,872 |
| walk itemList, 100 context maps | 67.5 | 67.6 | 1.00 | 806 → 706 |
| 100 objects made and walked | 21.8 | 22.9 | 1.05 | 100 both |

All 59 rows: ×0.901. The heap did not move: test-apps' six large processes peak at 15, 44, 59, 76, 82 and 86 MB (R49: 16
to 83 MB), with the main thread's runtimes 7 to 33 MB of a peak and 29 to 32 runtimes live at most. The inline-cache
trim did not lower these peaks; where they go now is not yet profiled (a `MOQUI_SCRIPT_HEAP_LOG` run is next). Against QuickJS's last numbers (R48's table), walk itemList is ×1.15, cached finds ×1.04 and the
locked template ×0.87.

The fork (each a `perf/` branch merged into `main`):
- **Numbers to text** (c83824fd): composed on the stack; integers from a `u64`, short decimals by one multiply, the rest
  by Rust's shortest `{:e}`; checked against the old walk over 10M doubles once (40k by default).
- **One-character strings** (2021d373): a per-runtime table of the 256 Latin-1 units, made on first use.
- **`replace`, `indexOf` and `includes`** (b33d2ebb) search a string where it lies.
- **Native `name` and `length`** (949b405e) pending until read: a fork runtime's start −8.8% instructions; no change in
  mr-moqui's start.
- **Regexp loops** (4c4e70de): a greedy one-character loop is one instruction with one backtrack record, none when its
  follower cannot match its characters; `FirstUnits` looks through loops. Differential test against split loops; the
  Test262 RegExp vector unchanged. Lazy loops and zipp's failed-run skip are not done.
- **A shared one-byte builder** (f29def49, with 3bcbe5a6 restoring the regexp files that merge undid) for join, pad,
  replace and case conversion: 6 to 14% fewer bytes in the template rows.
- **`JSON.stringify`** (7e665004) walks an ordinary object by a per-shape plan of slots and quoted keys, skips the
  `toJSON` lookup on an ordinary prototype chain, writes bytes: −15% instructions.
- **Inline caches** (80a91e10): a collection trims the cache store and compacts dynamic units' caches; a probe of 4000
  one-off units: 124 KB → 25 KB. js_speed keeps all its units, so no change there.
- **`JSON.parse`** (5a601429): integers of up to 15 digits into a `u64`, short keys and numbers from a stack copy:
  −9% instructions, the locked snapshot 124 → 42 allocations. Scanning a Latin-1 source in place and key plans are not
  done: the parser is UTF-16 throughout.
- **The lend** (362cf9ed): `Runtime` holds its `Heap` and `Services` in boxes, so a lend swaps pointers: cached finds
  −11.4% instructions, walk itemList +1.1%, 100 objects +1.7%.
- **Clippy** (1cdb77e2) on Rust 1.99.

mr-moqui:
- **Rows** (c497d59a): `$esc`, the locked snapshot's `JSON.parse`, numbers to text, `split('')`/`charAt`, a regexp
  scan; the engine start a row.
- **Release profile** (dd276100): one codegen unit for the three engine crates, ×0.994 over the rows at no build cost.
- **Templates** (bbdd62c5): one `$o.push` of a template literal for each run of output (a `+` chain makes an
  intermediate at each `+`, as `ConcatN` fuses only constants after the first); the compiled cache looked up by `&str`.
  fancy −26% instructions, locked −8%.
- **Tracked values** (2c426aef): a flush compares numbers, flags and nulls in place: walk itemList 806 → 706
  allocations.
- **Leaf finds** (f6479132): `ec.entity.find` and `condition` lend only for a definition made from rows: −5.6%.
- **Locked templates** (91fd453d): the snapshot's JSON written straight from the value; the sandbox's function found by
  the compiled text's address: −8.5%, 495 → 350 allocations.
- **Runtime start** (361f4c8e): the framework's start texts (shims, helpers, globals, ec's extras, the freeze) run from
  the process's code images after the first runtime: 8.67 → 1.54 ms, 47,455 → 9,277 allocations, 12.3 → 2.9 MB.
  The images cost the process about 0.7 MB once.

### The script heap

The gate (Oxide §7, Phase 3) is under half of 139 MiB of reserved runtime heap per process. R49 measured what the peak is
made of:
- **Workers' runtimes.** At mr-mantle's peak of 113 MB, 23 service-pool workers' runtimes held 87 MB, 2.9 to 5.3 MB
  each. A condition variable wakes any worker, so every worker ends up with a runtime.
- **A fresh runtime** reserves 0.87 MB (0.41 MB live). Its start costs 1.57 ms and 9,277 allocations since R50.
- **A worker's runtime** reserves 1.5 MB at the median and 5.4 MB at the most. The largest held: strings 1.6 MB, mostly
  atoms (11k of them); the tables beside the arenas 1.1 MB (37k inline-cache entries and the atoms); the value heap
  0.86 MB; objects 0.77 MB.
- **Compiled code** is not in these numbers: a runtime loads it from the process's images, outside its heap.
- **The main runtime's value heap** reserves 6.0 MB for 0.37 MB used: a few live ranges near its end hold it, and arenas
  give back only their ends.

R49's change: a service worker idle a second, while two others are idle too, lets its runtime go
(`script::release_idle_runtime`). Its next script starts one again: 8.4 ms and 47k allocations in R49, 1.57 ms and 9.3k
since R50.
- **mr-mantle:** the peak went 116 → 96 MB, with the suite time unchanged.
- **test-apps:** the process peaks went to 16 to 83 MB (from 91 to 118).
- **Earlier:** R44 drops a runtime whose reserved heap is more than 2.5 times its live bytes, at most every 30 s, and R45
  confirms that with a collection.

### Startup and threads

A first start takes 5.63 s and a restart 268 ms. The table check is 2.1 s of a first start, and initdb 1.8 s (R9). A
cached PK find scales to two threads, then stops: the slots lock's reader count and the `MCache` entry's atomics (R7).

## 4. What remains

**Database and driver**
- MySQL: the idle check, `sslMode`/`useSSL` roots, and `execute_deferred_row` for multi-row upserts (R5, Next steps).
- Poolers: untested. Transaction pooling needs PgBouncer 1.21+ with `max_prepared_statements` (R5).
- SCRAM-SHA-256-PLUS channel binding: hardening only (R5).
- Deferred writes on a cold statement still prepare with a waited round trip (Further work 3).
- Binary parameters for integers, timestamps and numerics (Further work 4).
- Nested data records go through an MNode per top-level element; a streaming builder could read them as `parse_flat`
  does (Further work 6).
- Reads before write for entities that cannot upsert (audit-logged, status flows, feed entities) cost a round trip each
  in a load (Further work 7).
- Profile `./moq load` end to end: its writes are well under 0.5 s of about 4.3 s (R7).
- A comparison with Moqui Java on the same Postgres, same calls and the same seed load (Further work 10).

**Concurrency**
- Cached finds past two threads: a per-thread memo of the definition lookups (R7, Further work 2).

**Startup** (R9)
- Create tables on several connections at once, foreign keys after.
- A feed probe costs three round trips; sent with its savepoint and release it is one.
- A template cluster would skip initdb's 1.8 s.
- Feed documents are indexed twice on a first start.

**The transaction cache** (TX §Gaps, §Results, §Later rounds)
- **Gaps still open:**
  - G9 is kept by design: a joined `cache` service holds for the rest of its caller's frame, so mark an operation's
    first service.
  - G5 catches only the held creates a find loads, so `DataGrid save#DataGridChanges` fails at commit.
  - G3 (the old row on a held update's touch) is still the plan.
- **Census failures left:** `Units store#UnitConversion`, the `BasicCommsEncrypted` poll, Chat search (TX §Census 2,
  §Reads).
- **The framework logic, left over:**
  - F1: the email's message id is written afterwards.
  - F2: the job passes' lock reads are unchanged, and not timed.
  - F3: session start and login are two frames.
  - F5: the per-touch re-read and walk (TX §Phase 5).
- **Not changed yet:**
  - the alert rule's finish (`moqui-report/src/alert.rs`, TX §Second set 11);
  - agent run events (`moqui-llm/src/run/events.rs`);
  - the `HttpSession` flush;
  - the feed receiver's delete per pending row (TX §Five pieces).
- **Early flushes left:** the services whose view reads of their own writes flush, `AssetServices`' `no-tx-cache`
  services, and `pack#Shipment` (TX §Census 5). Census 9's are data loads past the size cap and a bulk frame past the
  shadow's limit.
- **No byte cap yet** (the cap is 10,000 held writes), and creates are not regrouped across other writes (TX §Commit
  batch).
- **Later rounds:**
  - mr-mantle's order, shipment, invoice and payment flows, starting where Java's cache was taken off;
  - append-only entities always to the commit batch;
  - the cache on by default for some kinds of service;
  - MySQL multi-row inserts;
  - REST calls' own transactions (R26).
- **Risks:**
  - a held frame does not see other commits (TX D2, Decision 1);
  - locks are deferred to the commit, which breaks inserts used as locks (TX G8, Decision 2);
  - allocations are up with the cache on (S4 29k → 48k a call, TX §Reads).

**The script engine and screens**
- **The rows still behind QuickJS** (R50 profiles, frame pointers):
  - walk itemList (×1.15): the end-of-run compare is about 45% of the row, and 60% of that is
    `View::visit_plain_own_data` (own keys, then a get and a key lift each); a walk by shape members, as `JSON.stringify`
    now has, would fix it. `dense_array_values` makes an `Rc<Slot>` per element; `new_array_from_inits` defines each
    property one at a time.
  - cached finds (×1.04): `one()` still lends; a chained call still makes a new host object (the "already used" check,
    `entities_tests.rs`).
  - the locked template (now ×0.87): `$esc`'s replace is 45% of a render, and the `flags` getter 27%; the sandbox's
    frozen `RegExp.prototype` may keep it off the pristine path (not confirmed).
- **Not measured since F4:** a short compile (an expression from new text ×2.24, R31) (Oxide §6 F4, Zipp §2). HTML
  escape has a row since R50 (`$esc over 100 strings`).
- **A runtime start:** 1.57 ms and 9.3k allocations (2.9 MB) since R50, from 8.4 ms. With starts this cheap the idle
  release (`IDLE_RUNTIME`, 1 s) is worth re-weighing: a half second took mr-mantle's peak to 67 MB in R49.
- **Screens:** adjacent constant `sri.write` calls are not merged; they come from literal code in many macros, at most
  14 pairs a screen (R50).
- **In the fork:**
  - `fix/lower-heap-oom` (3f5d5809) is not in the pin; the bindings map the error already (Oxide §7, Phase 3 memory);
  - `using` and `await using` are a SyntaxError (the parser);
  - a back-reference is compared in one unpolled step (Oxide §6).
- **F1 step 3,** instructions shared by the process, and its open decision: unit-local key tables or process-wide atom
  ids (Oxide §6, §10). Also open: the process store's byte cap and when a text counts as one-off, which was to be set
  by the census, now gone (Oxide §10).
- **Older leftovers:**
  - bare-name writes still convert (R11);
  - script object keys and strings are one copy each (R18).

**The script heap, in the fork** (R49 "Next")
- Each runtime has its own atoms, about 11k in a busy worker. The design is on the fork's branch `perf/shared-atoms`
  (`docs/shared-atoms.md`, about two weeks, a process-wide `Sync` table), with `Runtime::atom_census`
  (`QJS_ATOM_CENSUS`). R50's counts on js_speed: 4,188 atoms on the main thread and 3,081 on a new one; builtin and code
  atoms are about 90%, string literals the largest class, host keys and the rest about 400. Count an mr-mantle worker
  before deciding.
- The inline-cache table: trimmed and compacted at a collection since R50 (80a91e10).
- A compacting value heap: the arenas give back only their ends.
- Gate status: the three largest test-apps processes are 3 to 10 MB over it (R49).

**Engine differences to settle** (Oxide "Engine differences under `oxide`", "Phase 3: engine differences")

Each is to be fixed in the fork or listed in [Parity-Diffs](Parity-Diffs.md):
- `using` and `await using`;
- `performance.now()` counts whole milliseconds;
- `Error.captureStackTrace` and `Error.prototype.stack`;
- stack columns (oxide at the call's `(`) and native messages (`stack overflow`, `cannot convert to object`);
- a decimal re-read after a releasing flush is a new object;
- the frozen helper globals (`ObjectUtilities`, `System`, ...) win over a context value of the same name, where QuickJS
  read the stack first.

**Allocation and CPU leftovers**
- `Value::clone` and the maps built to call services (R15).
- Allocation (12 to 13%) and `memcmp` (3 to 3.7%) in a service call (R27, R29, R30).
- OpenTelemetry attribute-set hashing and `IndexMap`s of values (R29).
- Span names copied, and the transaction span's attributes set per call (R21).
- A data-feed touch keeps two `String`s; an entity with cached finds keeps the values written (Further work 9).

**Dependencies**
- `croner` brings derive_builder, darling 0.20 and strum, about eight crates. Replacing it means matching Quartz's `L`,
  `W` and `#`. It is left for a decision: the audit after Phase 4, in the TOTP commit 5cba8100.

**Tests**
- Check day-keyed snapshots on a rebuilt binary, and why `test-apps` once kept a binary older than its sources (R30).

## 5. Ideas for improvements

Tiers by cost and evidence. A fork item is a branch of the fork, measured from mr-moqui behind a switch, then the Sweep
and `gates.sh`. An mr-moqui item is proved locally and committed.

Tiers 0 to 2 were built in R50 ([Round 50](#round-50-tiers-0-to-2)), apart from the items marked open below.

**Tier 0: measure first** (Zipp 0), done in R50
- `js_speed` rows for `$esc` over 100 strings, the locked template's `JSON.parse`, 1000 numbers to text, and the engine
  start as a row with time and bytes. Every tier 1 and 2 item needs a row to show it.

**Tier 1: cheap, with clear evidence**, done in R50 except the rope threshold (§6)
- **Release profile:** `codegen-units=1` for the oxide crates, then thin or fat LTO. zipp recorded about 2%; measure the
  build time too (Zipp 1).
- **Numbers to text** (the fork's `radix.rs`): a short-decimal path in place of format-and-parse, written into the
  caller's buffer. zipp 0d2445f3, 1.08G → 0.70G instructions (Zipp 2).
- **One-character strings without an allocation:** a per-runtime table, for matches, `charAt`, `split("")`,
  `fromCharCode` and JSON. zipp B62, JSON −6.7% (Zipp 3).
- **`replace` with a callback:** a no-match subject returned as is, and one-character matches from the table (Zipp 4).
  It builds on R43's `replace_pristine`. Rows: `$esc` and the locked template.
- **The rope threshold:** `ROPE_MIN` 64 → 128 or 256. zipp's sweep gave −25% on a regex log scan, but at 2048 one case
  was eight times slower (Zipp 5).
- **The template generator:** one `$o.push(a+b+…)` per straight run of output, so `ConcatN` joins it in one allocation.
  zipp B124 (Zipp 6). Check the screen generators too.

**Tier 2: aimed at the losing rows and the heap**, done in R50 except: `JSON.parse`'s in-place Latin-1 scan and key
plans; the regexp's lazy loops and failed-run skip; shared atoms (designed, not built) and the compacting heap
- **A one-byte string builder:** Latin-1 until a wide unit arrives, for `join`, `JSON.stringify`, `concat`, `repeat`,
  `pad` and case conversion. It removes the narrowing scan R43 found (Zipp 7).
- **`JSON.parse`:** scan Latin-1 in place, build each object at its final shape (zipp 57bbb00f, −11.9% instructions),
  and read integers of up to 15 digits into a `u64` (Zipp 8). Row: the locked template.
- **`JSON.stringify`:** walk directly when no `toJSON` is reachable, and cache each shape's slots and quoted keys. zipp
  bb6eded2 and 43693b4e, −19% instructions (Zipp 9).
- **Native `name` and `length` from the native table:** one pending shape in place of about 600 properties a realm. It
  lowers each runtime's start and floor, which about 30 runtimes multiply (Zipp 10).
- **The regexp executor:** single-character loops, possessive loops (zipp B116, −98.9% backtrack pushes), lazy
  look-ahead, and a literal or set scan over Latin-1 (Zipp 11).
- **A cheaper runtime start:** 8.4 ms and 12 MB allocated (R49). Profile it with frame pointers (DWARF did not unwind
  it). A cheaper start lets the idle release be shorter: at a half second the mr-mantle peak was 67 MB (R49).
- **The heap in the fork:** shared atoms, the inline-cache table shrunk after its units go, a compacting value heap
  (R49). F1 step 3 goes with the first.
- **A lend without the swap:** each lend of the engine to a non-leaf host call swaps the 4.2 KB `Heap` and the
  `Services` by value, about 10% of the cached-find row; a pointer swap needs the interpreter to reach the heap through a
  box (R36, noted, not done).
- **The three ×1.16 rows in the bindings:** a host map's conversion, a find's arguments and the snapshot's copy, from a
  new profile (R48).

**Tier 3: later**
- **PGO builds:** zipp B114, −13% overall, −10 to −30% where the interpreter dominates (Zipp 12).
- **The dispatch loop,** after a new qvt profile, within the loop size gate (Zipp 13):
  - fused instruction pairs (−5%; zipp's triples lost 9 to 20% elsewhere);
  - frameless leaf calls (B117);
  - `typeof x === "lit"` as one instruction (B54);
  - `x == null` with no heap read.
- **`using` and `await using`,** with zipp's grammar and DisposeResources as the reference (Zipp 14).
- **A generational GC:** the fork's `docs/planned.md` §6, with zipp's NURSERY_DESIGN.md to read (Zipp 15).
- **A JIT,** a distant option: deadlines by step loans per block, `MAP_JIT` on macOS and none on iOS (Zipp 16).
- **The database side:**
  - binary parameters;
  - the per-thread definition memo;
  - table creation in parallel at a first start;
  - a template cluster;
  - the transaction cache on by default for some kinds of service (Further work, R9, TX §Later rounds).
- **QuickJS for trivial actions:** a native path for `<set>` that would change value types, so it needs a decision
  (Further work 8).

## 6. Measured and dropped: do not retry

| Idea | What it showed | Proof |
|---|---|---|
| Hold deferred writes until 8 KB is buffered | 0 to 14% slower | R5 |
| COPY through a staging table | saves nothing; direct COPY saves 40% only for rows known to be new | R7 |
| Write an entity's whole column list to batch seed rows | clears the fields a record leaves out | Next steps 3 |
| A statement cache of 100 | re-prepares; it is 256 | R6 |
| `indexOf` in place of a regex in the screen macros | slower | R12 |
| Rust services taking their parameters by value | moves the copy only | R18 |
| A QuickJS context per nesting depth | little saved | R28 |
| Snapshots of the helpers | not possible in QuickJS | R10 |
| Chunked (paged) arenas | objects occupy 40 of 46 chunks; 10 to 15% back for an indirection on every access | R44, Zipp §4 |
| A runtime's start slack given back (`shrink_to_fit`) | measured, dropped | R45 |
| Inline small strings | 2% at most: seven inline bytes would take 14% of a qvt render's short concatenations | R46 |
| A smaller static code cache (2 MiB, 1 MiB of source) | more transient compiles; peak 116 → 141 MB at 1 MiB | R49 |
| An idle release after 2 s or 10 s | releases almost nothing in the suites; a half second releases 426 runtimes but starts one for most calls | R49 |
| zipp itself as the engine | one host channel, no re-entry, no shared code, 2 to 6.6 times QuickJS's memory, a JIT panic aborts | Zipp §1 |
| `codegen-units = 1` plus thin LTO for the release profile | ×0.988 on the screen rows over one codegen unit alone (under the 1.5% bar); a rebuild after a moqui-core edit 12% slower | R50 |
| `ROPE_MIN` 128 or 256 (from 64) | neutral to worse on every row: concatenations +0.7%/+2.1%, qvt rows +0.9%/+1.1% | R50 |
| zipp's regex engine, WTF-8 strings, its object layout, a thread that frees large payloads, a `"prefix"+int` cache | listed as not taken | Zipp §4 |
| Java's `knownLocked` shortcut; Java's stale view reads | not kept, by design | TX D4, TX §How it works today |

## 7. Behaviour changes to know

**The transaction cache**
- In a cache frame (`transaction="cache"`, `force-cache`), a row the frame read is not read again: another
  transaction's later commit of it is not seen until the frame ends (TX D2).
- Held writes go to the database at the commit, merged per row. The commit batch is answered by the commit's wait, and a
  held write's failure is the commit's error.
- A held update of a row read under a lock gets no changed-row check; one of a row not locked goes out as
  `WITH ... UPDATE ... RETURNING`.
- A select-field find is answered from memory until the frame writes that entity.
- A row created in a frame reads back with every field, and a held timestamp at the database's precision.
- View, distinct and having reads, encrypted members and views built in code read held rows through shadows (Postgres)
  instead of writing them first.
- `entity_tx_cache=false` turns the cache off everywhere. `screen_tx_cache` is true by default (R30).
- `no-tx-cache` makes a frame read-through; a later `cache` call that joins it does not turn holding back on.
- A chat message is indexed for search after its commit, on a worker.

**The Postgres driver**
- SAVEPOINT, RELEASE and COMMIT are pipelined, and a COMMIT the server answers with ROLLBACK is an error.
- A cold deferred statement is prepared with text parameters. A deferred write's failure surfaces at the next statement
  or the end of the file. A data load's failed row is reported for its multi-row statement.
- `sslmode=require` encrypts without verifying; `sslrootcert` makes it verify. An unreachable database answers 503.

**Services, jobs, data and resources**
- An async call or job logs in through the entity cache.
- `send#WebPush` runs with no transaction and writes its outcomes in one batch afterwards. `expire#UserStatuses` writes
  users in batches. SMTP Inbox mail and its rules commit together.
- An upsert records a `create` hit and runs no update ECA; `create_or_update` of a new row records no `refresh` hit.
- A feed touch with no document keeps no values. A write with no cached find clears the entity's caches whole.
- `getLocationReference` reads size, time and children lazily. `EmailUserState` and `ResourceLock` are
  `cache="false"`.
- `moqui test --db snapshot` keys its snapshots by day.

**The script engine** (Oxide §7, Phase 4 result; R49)
- quickjs-oxide is the only engine; the `script_engine` setting is gone and an old value is ignored.
- Each thread has one runtime, shared by the scripts nested in a run; the test harness's specs have a runtime of their
  own. A runtime's compiled code comes from the process's code images.
- A service worker idle a second, with two others idle, lets its runtime go; its next script starts one.
- A runtime its heap leaves far past its live bytes is dropped and started fresh (R44, R45).
- Inert modules are kept for an outermost run (`script_module_cache`).
- A runtime's start texts are compiled by the process's first runtime and loaded from its code images by later ones
  (R50).
- `ec.entity.find` and `EntityFind.condition` run without lending the engine, except for a definition made from rows
  (R50).
- A list or map read by bare name keeps its host type. A decimal read twice is the same object until a releasing flush.

**Open question**
- `echo#Data` returns `timestampOut` as millisecond text, where Java returns a Timestamp: undecided (R11).

## 8. Known flaky tests

- `service::coverage::tests::calls_are_counted_only_between_enable_and_finish`: a process-wide recorder, races with
  tests in parallel (R29).
- moqui-core `tarpit_locks_and_check_views_are_read_once_until_a_lock_is_written`: a process-static authorization
  generation (R22, R25; adjusted in R25 and R28).
- moqui-host's XMPP walk (`xmpp_clients_sign_in_…`): "not in time" under a full run's load, passes alone (R29, Phase 4).
- moqui-git `mirror_fetches_and_opted_in_writes_raise_events` and `the_tree_refs_compare_and_merge_methods_through_services`:
  timing under a full run, pass alone (R48, R49).
- mr-boss `WorkCenters` load of a released operation: intermittent (R22, R27, R28, R49).
- mr-mantle `SupportAssignment` round robin: fails under both engines and alone ("Customer Service is not set up for
  Assignment Spec Org"), from the Customer Service P5 work of 2026-11-05. Needs a look outside performance (R48).
- Contacts, Consent and CustomerServiceAccess (state from earlier specs), ShopCart and SupportAccess
  (`record#Attempt`'s rate limit), `ProductsMethods` (its budget, from earlier data): R23, R25.
- mr-boss `Resources` team planner on a Sunday evening in the Americas: the spec's `monday()` is the next day's week
  (2026-11-09) where the server's `today` is still 11-08 (R50).
- `AccountingCollections` and `SupportCases` after 23:00 local time; the day-keyed snapshots (R30) likely fix them, not
  yet seen in a run (R28).

## 9. Sources

The archived files stay whole as the record. Code comments cite them by section:
- **[archive/Profiling.md](archive/Profiling.md):**
  - every round's measurements, changes and tables (Rounds 1 to 49);
  - the speed tests' original "How to measure";
  - the Results table;
  - the old "Next steps".
- **[archive/TX-Cache.md](archive/TX-Cache.md):**
  - the design (D1 to D6), the gaps (G1 to G9) and the candidates (S1 to S5, F1 to F5, N1 to N4);
  - the phases and the censuses with their numbers;
  - the decisions of 2026-11-05.

  Comments cite `plans/archive/TX-Cache.md Dn` and `Gn`.
- **[archive/Script-Engine-Oxide.md](archive/Script-Engine-Oxide.md):**
  - §1's measurements against QuickJS and Boa;
  - §6, the fork's changes F1 to F4;
  - §7's phase results and the engine differences;
  - §10's decisions and §11's map of the code.

  Comments cite its §n.
- **[archive/Script-Engine-Zipp-Ideas.md](archive/Script-Engine-Zipp-Ideas.md):**
  - why not zipp;
  - the target rows;
  - the items of §5 above, with their zipp commits and B-numbers;
  - what was not taken.
- **[Performance-Summary](Performance-Summary.md):** the digest of the performance plan and the Mantle USL audit.
- **[Performance](Performance.md):** the targets.
