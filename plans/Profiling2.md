# Performance: where it stands, what remains, and the ideas

Status 2026-11-10 (the numbers are R53's, 2026-11-09). [Profiling3](Profiling3.md) reads these numbers again with
the code beside them and holds the next plan: what to measure, the small certain work and the larger work. This file
replaces four running logs, now in `archive/`:
- [Profiling](archive/Profiling.md): Rounds 1 to 49.
- [TX-Cache](archive/TX-Cache.md): the transaction cache, Phases 0 to 6 and Censuses 1 to 9.
- [Script-Engine-Oxide](archive/Script-Engine-Oxide.md): quickjs-oxide in and rquickjs out, Phases 0 to 4.
- [Script-Engine-Zipp-Ideas](archive/Script-Engine-Zipp-Ideas.md): ideas from zipp.org.

It keeps the latest numbers, what is still open and the ideas not yet built. Round 50 (2026-11-08, [below](#round-50-tiers-0-to-2))
built §5's tiers 0 to 2, Round 51 ([below](#round-51-shared-atoms)) the shared atoms, Round 52
([below](#round-52-the-boundary-rows-and-native-sets)) the boundary rows and native sets, and Round 53 (2026-11-09,
[below](#round-53-full-screen-renders)) full renders of real screens over HTTP. Rounds 54 and 55 (2026-11-10) are
Profiling3's instruments and weeding. Each one cites where it was measured or proposed: "R*n*" is a Profiling round, "TX §*name*" or "TX D*n*" a section or decision of TX-Cache, "Oxide §*n*" a
section of Script-Engine-Oxide, and "Zipp *n*" an item of Script-Engine-Zipp-Ideas §3.

Most of the database and service work is done:
- The entity, service and data-load paths have been profiled and optimized against embedded and external Postgres.
- The transaction cache holds writes and serves reads for 60-odd framework and application services. It took the
  waited round trips of the 45 services it was first applied to down 36% (R25), and the Mantle operations 35% (R27).
- The script engine is quickjs-oxide alone (Oxide §7, Phase 4 result). Its screen rows are within ×1.10 of QuickJS's
  last numbers, and plain JavaScript is faster than it was (R48).

No js_speed row loses to QuickJS's last numbers since R52: walk itemList ×0.94, cached finds ×0.91, the locked
template ×0.56. The heap of the script runtimes is under its gate in every test-apps process (the largest 62 MB of 73,
R52). It rises as the apps add code and data.

Not measured yet ([Profiling3](Profiling3.md) §2 and §4): an operation's round trips by kind, its CPU apart from
them, anything but the entity rows on more than one thread, and the process's memory beyond the script heap.

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
| Script engine | js_speed's 59 rows ×0.913 of R51 (×0.90 of R49 in R50); a simple `<set>` ×0.16 to ×0.20, `echo#Data` ×0.71, the locked template ×0.63, walk itemList ×0.82, cached finds ×0.88 | R52 |
| Runtime start | 1.42 ms, 6,182 allocations (R50: 1.57 ms, 9,277; R49: 8.96 ms, 47,436) | R51 |
| Script heap | test-apps process peaks 15 to 62 MB, all under the gate of 73 MB (with a 0.5 s idle release; 77 MB at 1 s); mr-mantle 65 → 54 MB | R52 |
| Screen renders | Tools Entities (1,482 rows) html 92 ms, qvt 153, tui 80 (from 196, 263, 287); Cache List html 9.0 ms; Moqui Java's Cache List 7.6 ms warm | R53 |
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
| `js_speed` | expressions, scripts, actions, services (with Profiling3 M3's `body#Large`, `call#Context`, `loop#Calls`), templates, screens, a new thread's first script; `JS_SPEED_ONLY=threads` the M4 thread rows | `JS_SPEED_ONLY`, `JS_SPEED_ROW`, `JS_SPEED_CALLS`, `JS_SPEED_BATCHES`, `JS_SPEED_DUMP`, `JS_SPEED_PEEK`, `JS_SPEED_THREADS` (`--features embedded-postgres`; `ic-stats` adds the inline-cache report after each section) |
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
  - `MOQUI_TX_CACHE_CENSUS_FILE` writes a line per transaction, with `micros` since R26, and since R54 its statements
    by shape (`transaction/shapes.rs`): each shape's head, kind, count, waited round trips and rows, the waited
    statements by kind, and `repeats`, `repeats_other_params` (a find in a loop) and `read_after_read`.
  - The Mantle operations are timed with `-Dentity_tx_cache=false` and `--db existing` against a template database
    (R26).
- **The script engine:**
  - `MOQUI_SCRIPT_STATS_FILE=<file>` appends a JSON line per process: scripts by kind, runtimes started, live and
    restarted, the heap at its peak, and `heap_at_max_by_runtime`, each runtime's share of the peak by thread (R49).
  - `MOQUI_SCRIPT_HEAP_LOG=<file>` writes a line per spec with the main thread's runtimes' arenas, code and caches.
- **Screen renders** (R53):
  - `MOQUI_RENDER_PROFILE=<file>` on a server appends a JSON line per screen render: the total, and per phase (the
    URL, actions, units, each host method, their argument and result conversions, the flushes, the output) its count
    and inclusive and exclusive time (`telemetry/profile.rs`). `MOQUI_OP_PROFILE=<file>` does the same for each
    outermost service call outside a render (R54, Profiling3 M1). Both lines also have the thread's CPU time
    (`cpu_us`), its waited round trips, counts of bare-name and entity-field reads and writes, and the artifacts
    pushed by type and action. A unit's exclusive time is the interpreter between host
    calls. Its clock reads add 5 to 15% to a render with many host calls; off, it is one static read a call.
  - The bench repository's `rounds/r53/`: `screen_bench.py` times screens over HTTP as john.doe (3 × 100, single
    threaded, `perf stat` of the server's instructions), `table.py` compares runs, `compare_bodies.py` diffs the
    outputs, `profile_table.py` sums a render profile, and `inclusive.py` gives inclusive shares from `perf script`.
  - `MOQUI_UNIT_DUMP=<dir>` writes each screen unit's generated source when it is made, named by screen and section, and
    each mode runtime as `runtime-<id>.js`: what a render runs, to read beside the macros.
  - The screen tarpit (`AT_XML_SCREEN`, 120 hits a minute in the demo data) answers a loop with error pages: turn it
    off in the conf for a timing (`<artifact-execution type="AT_XML_SCREEN" tarpit-enabled="false"/>`).
  - A round's changes go behind a temporary `MOQUI_OXIDE_OFF` switch, so one release build measures each
    (`rounds/r33/run-variants.sh`, `table.py` in the bench repository).
- **The fork's bench repository,** [moqui/quickjs-oxide-bench](https://github.com/moqui/quickjs-oxide-bench), checked out
  as `../quickjs-oxide-bench` (its `README.md`; it was the `../oxide-bench` workspace until R50):
  - its tools: `tools/gates.sh`, `tools/run-sweep.sh`, `tools/mantle-pg/` and `tools/ngrun/`;
  - the Sweep and memory results (`sweeps/`), the gates and Test262 logs;
  - each round's scripts, notes and tables (`rounds/r32` to `rounds/r53`).

  Binaries, worktrees, the suites and traces go in its `work/`, which git ignores.
  - **The Sweep.** Build a base from the fork's current `main` (`work/base-<sha>`), then `tools/run-sweep.sh`:
    `bench.py sweep` and `bench.py memory` over `work/suite-matrix/manifest.json`, in a quiet window. The gates: each of
    the eight V8 v7 programs at most ×1.02 of the base's instructions, and `run_at` under 16384 bytes.
  - **`tools/gates.sh <worktree>`** runs upstream's CI steps, GC stress included.
  - **Merging.** A branch merges `--no-ff` into the fork's `main` with the Sweep's numbers in the message. The user
    pushes the fork, then mr-moqui's pin moves and `cargo update -p quickjs-oxide` runs.
  - No upstream issues or pull requests without asking.
  - **What goes in the fork** (the user, 2026-11-10): anything of high value to mr-moqui. Upstream taking a change
    is a plus and does not decide it ([Profiling3](Profiling3.md) §9 D4).
  - **`tools/mantle-ops/`** (R54, Profiling3 M3) runs the eight mr-mantle specs with the operation profile and the
    census shapes on, on the embedded Postgres or on `moqui-pg18`, and `table.py` gives each of the nine operations'
    wall and CPU time, round trips by statement kind, and the shapes most waited for.
  - **`tools/mantle-pg/`** times the nine Mantle operations against `moqui-pg18` 200 µs away: `setup.sh`, `run.sh`,
    `table.py`, `teardown.sh`. It ran two engines in turn; it now runs one.
- **Rules learned:**
  - Prove locally (the rows, a profile, the touched tests), then commit. The Sweep, Mantle runs and full suites are for
    an acceptance.
  - Nothing else builds while a timing runs. Run two of each and compare them at the better one.
  - A round's text results go in the bench repository's `rounds/rNN/`, and its binaries and traces in its `work/rNN/`.
    Not `/tmp` (a tmpfs filled up once), and not under mr-moqui's `target/`, which `cargo clean` removes (R50's raw
    outputs went that way).
  - Under low load, memory matters more than the last millisecond; under high load, speed does.
  - The rows are small and run on one thread: a service of a few lines, a short context. A cost that grows with a
    body's size, with what the caller's context holds or with the thread count does not show in them
    ([Profiling3](Profiling3.md) §2, "What the speed rows do not show"; its M3 and M4 add the rows).

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

quickjs-oxide, a QuickJS-compatible engine in safe Rust, is the only engine, pinned to the fork's `main` b7e34e04
(the engine differences settled, 2026-11-09; f873d7e7 in R53, the sampling profiler; ddba97fe in R51, 1cdb77e2 in
R50, 7cf1f530 before, Oxide §7, Phase 4 result).

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
(the raw outputs were lost to a `cargo clean`; see the bench repository's `rounds/r50/README.md`):

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

R49's change: a service worker idle a second (half a second since R52), while two others are idle too, lets its runtime
go (`script::release_idle_runtime`). Its next script starts one again: 8.4 ms and 47k allocations in R49, 1.4 ms and
6.2k since R51.
- **mr-mantle:** the peak went 116 → 96 MB, with the suite time unchanged.
- **test-apps:** the process peaks went to 16 to 83 MB (from 91 to 118).
- **Earlier:** R44 drops a runtime whose reserved heap is more than 2.5 times its live bytes, at most every 30 s, and R45
  confirms that with a collection.

### Round 51: shared atoms

The fork's `docs/shared-atoms.md`. Atoms that every runtime would intern alike live once per process, in a shared table
that is read with no lock and added to under a mutex; they are immutable and never collected, with ids from 2^29.
A runtime looks in its own table first, so a text never has two ids in one runtime.
- **Phase 1** (66db6a8f): the 512 builtin atoms. A fresh fork runtime reserves 171 → 125 KB. Instructions +0.5 to
  +3% on the rows that make many strings (concatenation +3.0%, in glibc's `malloc`, not in the engine); the eight V8 v7
  programs within ×1.02 (geometric mean ×1.003).
- **Phase 2** (3c741ee8): a code image's keys, function names and string constants, as the image loads, up to a cap
  of 261k atoms and 16M units. `Runtime::atom_census` is an API (no print).

What the counts showed first (quickjs-oxide-bench `rounds/r51/census-mantle`): the busiest mr-mantle workers held 9.7k
to 9.8k atoms, 91% of them code (image keys about 5.0k, constants 3.7k to 3.9k), host keys about 230.

mr-mantle's specs, phase 1 against phase 2 (`rounds/r51/shared-atoms-p2`):

| | Phase 1 | Phase 2 |
|---|---:|---:|
| Busiest worker's atoms (its own) | 11,614 (11,102) | 13,302 (831) |
| Service workers' most reserved, median | 3324 KiB | 2230 KiB |
| The 23 workers' most reserved, summed | 70.8 MiB | 53.6 MiB |
| Runtimes at the process peak (`heap_at_max`) | 99.7 MB | 75.4 MB |
| The shared table | 512 atoms | 28,858 atoms, 4.2 MB once a process |

js_speed: a new thread's runtime reserves 860 → 734 KiB (live 398 → 325) from R50, 1,132 → 863 KiB after a List
render; the engine start 9,277 → 7,775 allocations. The script rows' instructions are unchanged from phase 1: they run
on the thread that compiled the images.

test-apps' six large processes peak at 15, 36, 48, 53, 66 and 68 MB, all under the gate of 73 MB (R50: 15, 44, 59, 76,
82 and 86 MB).

**Phase 3** (b0ae1755): each image unit keeps one key table, taken by every runtime whose atoms for its keys are the
shared ones; a runtime whose own atom came first builds its own (`CodeUnit::keys` is an `Arc<[PropertyKey]>`).
Image keys alone left most of the workers' entries their own: the units name host keys (`ec`, host members, entity
fields) that each worker had already interned before its first image load.

**Declared keys** (34ef6147, mr-moqui `engine.rs`): the embedder shares the keys it declares, and no others.
`Runtime::share_keys` and `set_share_atoms`: mr-moqui shares every atom made during a runtime's start (`ec`, the host
table and objects, the shims, the helpers) and, once a process, the entity model's entity, field, alias and relationship
names. Keys from run-time data, such as a parsed map's keys, stay the runtime's own and are collected; sharing every
key the host made, the first try, let data keys fill the never-collected table up to its cap.

mr-mantle, image key tables alone (k1), every host key shared (k2, not kept) and the declared keys (k3, quickjs-oxide-bench
`rounds/r51/shared-key-tables`, `rounds/r51/declared-keys`):

| | k1 | k2 | k3 |
|---|---:|---:|---:|
| Workers' key entries in tables of their own | 304,733 of 416,046 | 128,844 of 419,528 | 18,933 of 417,420 |
| Workers' most reserved, median | 2289 KiB | 2284 KiB | 2177 KiB |
| Workers' most reserved, summed (23) | 54.9 MiB | 52.9 MiB | 52.0 MiB |
| The shared table at the end | 28,281 atoms, 3.2 MB | 31,081, 4.3 MB | 34,525, 5.7 MB |

k3's table starts at 11.6k atoms (the start and the model) and grows only as new code images load. A new thread's
engine start: 6,166 allocations (7,775 after phase 2), 1.41 ms; its runtime after List html 808 KiB reserved.

test-apps after the localization merge (2d0e1f29; quickjs-oxide-bench `rounds/r51/test-apps-heap`): the largest
process peaks at 80.7 MB on 3c741ee8 and 76.8 MB on 34ef6147, against the gate's 73 MB; the other five are 13 to 71
MB. The growth is the tree's, not the engine's: what that merge added to the largest process's main runtime and
workers is the next thing to look at.

### Round 52: the boundary rows and native sets

The quiet pass, the round's base (mr-moqui fb0e5f78 on fork 34ef6147) against e36b0c9b on ddba97fe, the best of two
runs (quickjs-oxide-bench `rounds/r52/final`): ×0.913 over the 59 rows.

| Row | Base µs | R52 µs | ÷ | Allocations |
|---|---:|---:|---:|---|
| one set | 0.93 | 0.19 | 0.20 | 6 → 2 |
| four sets (the echo#Data body) | 2.91 | 0.48 | 0.16 | 26 → 4 |
| BasicServices.echo#Data | 11.79 | 8.40 | 0.71 | 120 → 94 |
| echo#Inline | 13.35 | 10.49 | 0.79 | 144 → 121 |
| fancy, locked down | 62.22 | 39.01 | 0.63 | 350 → 228 |
| walk itemList, 100 context maps | 67.29 | 55.02 | 0.82 | 706 → 406 |
| 10 noop service calls, itemList tracked | 187.07 | 163.24 | 0.87 | 1,931 → 1,331 |
| 10 cached entity finds | 45.13 | 39.87 | 0.88 | 576 → 486 |
| find#Cached | 56.38 | 50.78 | 0.90 | 657 → 567 |
| loop#Actions | 340.94 | 306.97 | 0.90 | 3,390 → 2,475 |
| iterate 100 context maps with if and set | 260.71 | 233.44 | 0.90 | 2,500 → 1,675 |

The fork:
- **F1** (570f5b59): a frozen `RegExp.prototype` whose methods are untouched stays pristine. The sandbox's lockdown
  froze it, which gave it a new shape, and the fast paths compared the shape: the locked template −38.9% instructions.
- **F2** (c64d537d): `visit_plain_own_data` reads a plain object from its shape and slots; new
  `visit_plain_own_text` and `visit_dense_array` show keys as text and elements in place. walk itemList −13%.
- **F3** (ddba97fe): a back-reference compare counts toward the regexp interrupt poll. The regexp row flat.

mr-moqui:
- **Native `<set>`** (9629dcdb, `script/actions_set.rs`): an actions body of only simple `<set>`s runs as a plan with no
  script engine. A literal or `${name}` value, a bare-name, dotted or literal `from`, `type`, `default-value` and
  `set-if-empty`; anything else, or a value the plan cannot answer as the script would, runs the generated script.
- **Java's `<set>` rules** (e36b0c9b): `default-value` where Groovy's `?:` takes it, and `set-if-empty="false"` where
  `ObjectUtilities.isEmpty` says empty, in the plan and the macro alike.
- **Cached finds** (2ffd2725): a chained `EntityFind` call returns the same find, as Java's builder does, and a find
  runs again; `list`, `one` and `iterator` answer a shared-cache hit without lending the engine. −11.4% instructions.
- **Tracked values** (e7041354): the end-of-run compare and the conversion use F2's visitors.
- **The OOM translation** (6304f480) is gone: the fork reports a compile that runs out of heap as `OutOfMemory`.
- **The idle release** (ef319839): `IDLE_RUNTIME` 1 s → 500 ms. test-apps' largest process 75.5 to 76.9 → 56.9 to
  58.2 MB, the processes' sum 317 to 363 → 269 MB, runtimes started 910 to 1022 → 1861 to 1983, the suite time 263 to
  279 → 258 to 270 s; mr-mantle 65.2 → 53.5 MB (`rounds/r52/m1`).

**Code placement.** The pass showed +7 to +19% on rows with no host work (1000 adds to a local, 100 objects, Plain.qvt)
with the instructions flat. The bisect (`rounds/r52/layout-bisect`) put it on where `vm::interp::run_at` starts in its
cache line: every build with it at offset 0x30 ran the adds row about 10% slower, whatever the edit, and the final
source built another way ran at the base's speed. Wall times of the interpreter rows move about 10% with unrelated
edits; read instructions for them, and PGO (§5, tier 3) would steady them.

### Round 53: full screen renders

The first comparison with Moqui Java (2026-11-09, one thread, 3 × 100 renders each, as john.doe on demo data):
System / Cache List 9.4 ms against Java's 7.6 ms once its JIT is warm (9.5 ms in its first round), with output of 66
against 112 KB, and every round flat where Java's had 100 to 400 ms outliers. This round renders five tools screens
over HTTP on the release server, splits each render by phase, and takes the largest costs out. The pages use page
parameters for long runs: Entities (`pageSize=3000`, 1,482 rows, three links a row), Cache List, Cache Elements of
`l10n.message` (330 rows), Artifact Stats (`levels=3`) and the System dashboard, each in html, qvt and tui.

Medians of the third round and `perf stat` instructions a request, base against the end of the round (bench
`rounds/r53`, `work/r53/{base,m3}.log`). Wall times of the small pages move up to 20% between builds with their
instructions flat (the code placement of R52); read the instruction ratios. Artifact Stats lists every screen and
service, so it grew with the System Conf screens while this round ran:

| Screen | html ms | qvt ms | tui ms | html Mi ÷ | qvt Mi ÷ | tui Mi ÷ |
|---|---:|---:|---:|---:|---:|---:|
| Entities, 1,482 rows | 196 → 92 | 263 → 153 | 287 → 80 | 0.51 | 0.61 | 0.26 |
| Cache List | 12.0 → 9.0 | 13.0 → 9.7 | 11.6 → 4.9 | 0.73 | 0.75 | 0.36 |
| Cache Elements, 330 rows | 23.6 → 17.5 | 28.6 → 20.9 | 27.9 → 11.0 | 0.75 | 0.78 | 0.32 |
| Artifact Stats | 160 → 94 | 179 → 110 | 176 → 81 | 0.51 | 0.55 | 0.40 |
| Dashboard | 6.7 → 6.5 | 5.3 → 5.0 | 7.4 → 4.4 | 0.92 | 0.89 | 0.45 |

Outputs are the same as before, live values (memory, hit counts, times) aside.

**What the render profile showed** before the changes, Entities html (223 ms with the profile on): 51% host method
bodies, 38% units, 9% argument and result conversions, 1% actions. 108k host calls a render. The links cost most:
`UrlInstance.getDisableLink` 5.1 µs a call, twice a link in html (45 ms), and `sri.makeUrlByType` 7.0 µs a link (31
ms). The perf profile put 22% of the render in `disable_link`: its `permitted` check pushes the path's screens on a
fresh artifact stack for each link, with telemetry spans on each push (14% of the render), and the path resolve read
`ScreenPathAlias` for each link (7.6%). In tui, 71% was units: `$clean` walked every key and value of the document
one code point at a time (`for (const ch of text)`, with `out += ch`), and `$scrub` rebuilt `JSON.stringify` in
script.

**The changes:**
- **A link's permission once a render** (`urls.rs` `permitted_in_render`): kept in the render state by the resolved
  path's address, for the user and authorization state it was decided under; a transition's condition still runs on
  each call. `getDisableLink` 5.1 → 0.47 µs.
- **A link path resolved once a render** (`resolve_path`), by the screen path it is resolved from and its text, and a
  url-type's localization once a render for a text with no `${` (`url_type_of`). `makeUrlByType` 7.0 → 4.0 µs.
- **The tui writer on `JSON.stringify`** (`ScreenTui.js`): a replacer keeps `$scrub`'s `null` for `undefined` and text
  for a function, symbol or bigint; one regexp test over the finished JSON finds the escaped controls, and only then a
  replace drops them (tab and line feed kept, as before).
- The render profile (`screen/profile.rs`), and js_speed rows for a tui list and a list of links:

| Row | Base µs | R53 µs | ÷ | Allocations |
|---|---:|---:|---:|---|
| List, tui, 100 rows | 4114 | 2336 | 0.57 | 37,542 → 22,356 |
| Links, html, 100 rows | 9811 | 8522 | 0.87 | 80,726 → 66,064 |
| Links.qvt, 100 rows | 12636 | 11277 | 0.89 | 84,078 → 69,401 |
| Links, tui, 100 rows | 11124 | 3753 | 0.34 | 101,027 → 36,167 |

js_speed runs with authorization off, so its links gain less than a signed-in user's.

**The macro pass** (`MOQUI_UNIT_DUMP` to read the units). A list cell's unit code is thin (push a context, call the
runtime's `display` or `link`, pop); the time is in the runtime functions. What came out, Entities instructions a
request against the first changes (`work/r53/m1.log`): html 1831 → 1438 M (×0.79), qvt 2617 → 2203 M (×0.84); in all,
against the base, html ×0.58, qvt ×0.68, tui ×0.27, outputs the same.
- **A link's attributes once for each element** (`linkOf`, as R12's `displayOf`), its disabled check once a link (html
  asked twice), and per render the texts with no `${` (link text, badge, tooltip, confirmation; `xc`) and the theme's
  icon for a text, kept with the unit stack so a render inside a unit keeps its own (html and qvt).
- **qvt's row values** (`fields-initial`): a widget's attributes once (`valueWidgetOf`), the link text per render, the
  plain value split only for the kinds that read its parts, and `fieldValues`, `jsAttr` and the cell loop on indexed
  loops. A probe with the row values left out put them at 312 M instructions of 2366 (13%) and their writing at 107 M.
- **html's row forms:** the token and hidden inputs every row's form has, made once a list.
- **In this engine** a `forEach` or `map` callback is a call, so indexed loops are cheaper in per-row code. A `for...of`
  over a plain array is walked by index already (the fork's pristine-array path); over a string it makes an iterator
  result for each character (the tui `$clean` loop).

**The second batch** (`work/r53/m2.log`):
- **A script sampling profiler** (quickjs-oxide `perf/js-sampler`, merged as 223a2047 and pinned, Sweep
  `r53-sampler`: V8 ×1.0000, memory ×1.005): a poll that finds the embedder's flag set counts the running stack as
  `name:line` frames (`Runtime::start_sampling`, `take_samples`). In mr-moqui `MOQUI_JS_SAMPLE=<file>` starts a 1 ms
  ticker and writes folded stacks under each render's path (`script/oxide/sampler.rs`); `samples_table.py` sums a
  file. The counts here were taken before the sampler was made honest (the third batch), so they lean to functions
  entered often. A poll comes
  at a call or a back-edge, so the counts lean to functions entered often; read the inclusive column. Entities html:
  `link` 45%, `w` (the writes) 17%, `rowCellOpen` 12%, `fid` 9%; qvt: `displayWidget` 26%, `rowValues` 15%, `fid` and
  `fname` 10% each; tui: `$addPart` 26%, `$writes` 13%, `$cellOf` 13%.
- **A form-list row written once** (html and qvt `bufferRows`): when every field's widgets write through the mode's
  own writer (display, display-entity, link, hidden, ignored, label, image; `ScreenSupport.bufferedListRows`), a row's
  writes gather in script and go out once at its end, also when a cell throws. About 34 host writes a row become one:
  −3.5 to −4.5% instructions on every list.
- **The known service names once a file** (`service/define.rs` `names_in_file`): kept by path with the file's
  modification time and length, so a listing reads only changed files. Artifact Stats asked twice a render, 36 ms
  each: its renders ×0.49 to ×0.54 instructions.
- **Looked at, not changed:** `makeUrlByType`'s URL build (a per-render memo of the resolved path would save about 1%:
  the URL cache answers it already); Artifact Stats' `openFormList` (9 ms a list) is its seven `from` expressions a
  row, three of them plain paths a native read could take, about 2%, but a native read keeps host types where the
  script gives text and the narrowest number, which a display shows differently; a `for...of` fast path (arrays are
  walked by index already).

**The third batch** (`work/r53/m3.log`; quickjs-oxide pinned at 223a2047, then f873d7e7):
- **The sampler made honest** (fork `perf/sampler-natives`, merged as f873d7e7, Sweep `r53-natives`: V8 ×1.0001,
  memory ×0.994; pinned at f873d7e7): while a sampler runs the loop polls at every call and back-edge (the counts follow time, where every
  thousandth event weighted them by calls), and a sample asked while a native runs is the native's (`name (native)`),
  where it went to the script that ran next. Entities html then: script 38%, host calls 42%, builtins 20%
  (`RegExp.test` 3.6%, `String` 2.4%, `Object.keys` 2.1%); qvt 46/30/25 (`test` 6.6%, `String` 4.3%); tui 31/50/19.
  The sampler costs nothing measurable with its poll at every event (Entities html 100.0 against 100.5 ms).
- **A link's field id and a qvt display's ids** from names read once an element (`linkFieldId`, `displayOf`), and
  html's plain row cell `<div>` as text: Entities html ×0.957, qvt ×0.967.
- **No context for a cell that sets nothing** (`ScreenSupport.cellNeedsContext`): Java pushes one for each list cell
  so its `set` elements stay in it; a part with no `set` whose widgets put nothing in the context (the
  `bufferedListRows` set) is written without the push and pop. With **`isAnchorLink` answered in script** for
  `link-type` anchor and anchor-button: html ×0.938, qvt ×0.961.
- **tui links:** a text with no `${` expanded once a unit (`$xc`), `isTransition` asked once, and no post check for an
  anchor: Entities tui ×0.969.

**Fancy templates** are compiled in Rust (`script/template.rs`: the tags, `${}` and the generated function, cached by
text) and `${}` in an expand is a template literal compiled once by its hash; nothing parses template text in script
on a render. `sri.renderText` was 0.6 ms of Cache Elements (four templates) and 0.2 ms of the dashboard.

**What is left** (Entities html after the changes, 171 ms with the profile on):
- **By the honest sampler** (Entities): host calls are the largest part, `makeUrlByType` (5.5% html, 9% tui),
  `startFormListRow` (5%), `getFieldValueTexts`, `getUrlWithParams`; then the builtins the escapes and conversions call
  (`RegExp.test`, `String`); tui's stringify replacer (`$plain`, 8%, one call a value).
- **`makeUrlByType` at 4 µs a link** (9% of html, 15% of tui): the `parameter-map` evaluated as a nested script (3.7%)
  and the URL's parameters built from the screen and transition definitions (3.2%). The macro could pass the map it
  already has in script.
- **`sri.write`** outside buffered rows: rows with other widgets, headers, and the hidden row forms of html.
- **The host boundary:** argument and result conversions are 11% of html.
- **No profile of the end:** `perf` was taken at the base and after the first changes, the second with the render
  profile on. Screen output has streamed since (0a1f1a41, 2026-11-10), so a rerun of `screen_bench.py` with a clean
  `perf` profile is the next base ([Profiling3](Profiling3.md) M5).
- **A row is still about 840 thousand instructions and 65 host calls** (Entities html). Profiling3 S4 proposes rows
  as templates the host fills.

### Round 54: the instruments, and the round trips named

Profiling3 M1 to M4 and M6 (2026-11-10; the commits name them). The operation profile (`MOQUI_OP_PROFILE`), the census's
statement shapes, the M3 service rows, the thread rows and the engine counters are in §2. M5 (a `perf` profile)
waited: `perf_event_paranoid` was back at 4 after a reboot. M7 and M8 are later. Results are in the bench repository's
`rounds/r54/`.

- **The nine Mantle operations, in process** (`tools/mantle-ops/`, embedded Postgres, medians): an operation's CPU is
  55 to 60% of its wall time on a local socket (place#Order 9.5 ms, 5.5 ms CPU, 66 waited round trips, 413 bare-name
  reads, 25 service calls, 73 entity operations; apply#Settlement 32 ms, 17 ms CPU, 860 bare-name reads, 386 entity
  operations).
- **Where the round trips go.** The largest single item was `SAVEPOINT moqui_feed`: 20.5 of place#Order's 66, and
  2,781 in the run. Each read of the data feed at commit went in a savepoint probe whose `SAVEPOINT` waited for the
  previous probe's `RELEASE`. They now go behind it (`probe_behind`): place#Order 66 to 46 round trips, approve#Order
  54 to 38, add#OrderProductQuantity 51 to 36, change#InvoiceState 80 to 57.5, create#Payment 77 to 50,
  apply#Settlement 120 to 100.5, pack#Shipment 117 to 84.5, approve#Payable 84 to 66.5. What is left is reads (32 of
  place#Order's 46: `order_header`, `order_part`, `entity_audit_log`, `order_item` the most), locking reads (8) and
  repeats of a shape (46 statements, 10 with other parameters, 20 reads right after a read): S1's material.
- **The pool's wrapper passed two calls on as the trait's defaults.** `Tracked` did not forward
  `execute_deferred_update` or `savepoint_behind`, so R18's update the server checks was never used through the pool:
  `tx_cache_speed`'s rt drops by one a call wherever a held update commits (S3 11 to 10, S4 14 to 13, N3 3 to 2).
- **First thread rows** (`JS_SPEED_ONLY=threads`, on a busy machine): noop#Inline 27 us a call on one thread and 5.7
  on 16, List html 1,667 to 418 us. Each thread starts its own runtime inside the row. To read again in a quiet window.

### Round 55: weeding

Profiling3 W1, W2, W4 to W10 and W12 to W17, and the operation metrics (2026-11-10), each in its own commit with its
rows. W3 is not built: a call's cleaned parameters are read again after the body (the post rules, the callbacks, the
tx rules), so they cannot be moved into its frame without changing those. W11 goes with S4a. `rounds/r55/table.md`
has the base (before W1) against the end of the round, best of two alternating runs:

| Row | Before | After | Allocations |
|---|---:|---:|---|
| noop#Inline | 7.98 us | 7.02 | 84 to 61 |
| call#Nested | 17.1 us | 16.0 | 169 to 143 |
| call#Context (30 names, 10 calls in-map context) | 482 us | 230 | 6,872 to 2,165 |
| entity-find-one cached, from actions | 4.56 us | 3.83 | 44 to 31 |
| 1000 adds to a bare name | 370 us | 312 | |
| four sets | 0.48 us | 0.36 | |
| loop#Actions | 301 us | 284 | 2,474 to 2,452 |
| body#Large (5 KB) | 13.1 us | 11.5 | 111 to 88 |
| fancy, trusted / locked down | 47.9 / 40.2 us | 32.2 / 25.8 | 244 / 228 to 121 / 106 |
| Plain.qvt | 30.7 us | 25.0 | |

- The targets of Profiling3 §5 are missed for now: noop#Inline is 7.0 us and 61 allocations (target under 4 and 40),
  call#Nested 16 (under 9), a cached find from actions 3.8 (under 2.5). The rest of a service call is its telemetry
  span (about 20 allocations, recorded whole and kept or dropped at its end, a design to decide) and the crossings of
  the generated chain (S3a); of a cached find, its crossings (S3b).
- `empty actions` reads 0.65 us in a run of every section and 0.28 alone, in this build only: a collection of the
  script heap lands in the row (it frees what the sections before made). It is not the row's own cost.

### Round 57: round trips (Profiling3 S1, first part)

- **`service_tx_cache`** (MoquiDefaultConf, false for now): a service with no `transaction` attribute, called with no
  cache setting, begins its transaction holding; entity-auto services do not. Over the eight mr-mantle specs: 4,777
  frames held against 1,475, no early flush, all 111 features, waited round trips ×0.89. The nine operations already
  held. It stays off until the final gate's suites run with it on (Profiling3 D3).
- **dbresource paths through the entity cache**, as Java's `findDirectoryId` reads them: `db_resource` reads 5,891 to
  4,208 in the run, get#PaymentFinanceDetail's 532 to 48, the data feed's 836 to 129.
- **Sized, not built** (from the census's shapes, `rounds/r57/`): place#Order's 46 round trips are 32 reads, 8 locking
  reads, a count and the commit; 46 statements repeat a shape, 10 with other parameters (what `= ANY` batching of a
  find in a loop could take), and 20 waited reads follow another read (what reads sent together could take). Reads of
  reference-like tables are 12% of the operations' round trips, most already through the entity cache; the largest
  shapes are Mantle's own (`order_header`, `order_part`, `entity_audit_log` for status history).

### Round 58: context names bound on the global object (Profiling3 S2)

Built in mr-moqui with no fork change: at a bare name's fourth access since the bindings were emptied, it becomes an
own data property of the global object, which the engine's global sites already cache; flushes write changed values
back, and a flush that may change the stack (and a nested run, around it) takes the bindings off. 1000 adds to a bare
name 310 to 13.9 us, iterate 100 context maps with if and set 210 to 79, loop#Actions 282 to 125: Profiling3's three
targets met. In Mantle bodies, which call the host often, reads through the hooks fall 8 to 30%.

### Round 59: the coarse crossings (Profiling3 S3a, S3b; S4 sized)

- **S3a:** a sync `service-call` is one `ec.service.callXml`; call#Nested 15.4 to 14.6 us, loop#Calls ×0.92.
- **S3b:** an `entity-find-one` with literal options is one `ec.entity.findOneXml`, and an auto-field-map of the
  context reads only the entity's fields; cached from actions 3.82 to 3.12 us (R55's base 4.56; target 1.5 missed).
- **The screen runtimes' escapes** with their patterns made once and no `String()` of a string: screens ×0.97.
- **Sized, not built:** a Links html render (6.8 ms) is 63% interpreter between host calls (17 a row), so S4a's one
  crossing a row cannot reach its target; S4b (row templates) is the lever. S5a (the host call floor) is about 0.5% of
  a list render, S5b's writes 311 a render.

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
- Nothing but `entity_speed` runs on more than one thread. The per-call path takes process-wide locks (the
  generated-actions cache, the artifact statistics, the tarpit map), and both pools wake every idle worker for a job:
  [Profiling3](Profiling3.md) §2, its W1, W6 and W13, and the thread rows of its M4.

**Startup** (R9)
- Create tables on several connections at once, foreign keys after.
- A feed probe costs three round trips; sent with its savepoint and release it is one.
- A template cluster would skip initdb's 1.8 s.
- Feed documents are indexed twice on a first start.

**The transaction cache** (TX §Gaps, §Results, §Later rounds)
- **Round trips nobody has classified:** a Mantle operation's time is its round trips (300 to 470 µs each at 200 µs
  to the database), and the frame counts only some of them as its reads: place#Order 21 of 66, pack#Shipment 45 of
  117, the rest but the commit unnamed (R48's census, read in [Profiling3](Profiling3.md) §2). Its M2 adds statement
  shapes to the census and its S1 lists what could then go.
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
  - the cache on by default for some kinds of service: to test, turned on for services with no setting of their
    own, with a rule for when such a service holds taken from what that shows ([Profiling3](Profiling3.md) S1, §9 D3);
  - MySQL multi-row inserts;
  - REST calls' own transactions (R26).
- **Risks:**
  - a held frame does not see other commits (TX D2, Decision 1);
  - locks are deferred to the commit, which breaks inserts used as locks (TX G8, Decision 2);
  - allocations are up with the cache on (S4 29k → 48k a call, TX §Reads).

**The script engine and screens**
- **walk itemList's read side** (R52 profile, `rounds/r52/m2`): `to_js_at` is 52% of the row, and 36% is
  `new_array_from_inits`: `View::object_from` makes each map's object and defines each property with a full descriptor
  (32%), and each text value gets a new string record. Build the objects by shape transitions, or one shape per key
  list as `JSON.parse` does. On mr-moqui's side, `to_init` could reuse the previous map's keys when a list's maps share
  them.
- **Not measured since F4:** a short compile (an expression from new text ×2.24, R31) (Oxide §6 F4, Zipp §2). HTML
  escape has a row since R50 (`$esc over 100 strings`).
- **A runtime start:** 1.4 ms and 6.2k allocations since R51, from 8.4 ms; the idle release is a half second since R52.
- **Screens** (R53, its "What is left"): a form-list row is written once when its widgets allow (`bufferRows`); other
  rows, headers and html's hidden row forms still write piece by piece. By the sampler, host calls are the largest
  part of a list render (`makeUrlByType`, `startFormListRow`, `getFieldValueTexts`), then the builtins of the escapes
  (`RegExp.test`, `String`) and tui's stringify replacer. Measure with `rounds/r53/screen_bench.py` and the
  sampler.
- **F1 step 3:** instructions are shared through the image bodies with unit-local key tables, and R51 shares the key
  tables themselves. Still open: the process store's byte cap and when a text counts as one-off (Oxide §10).
- **Older leftovers:**
  - bare-name writes still convert (R11);
  - script object keys and strings are one copy each (R18).
- **In [Profiling3](Profiling3.md):** bare names as bindings the engine caches (S2), a generated `service-call` or
  find as one crossing (S3a, S3b), action plans (S3c), the host call's fixed cost, inline-cache coverage and a mark
  for tracked objects (S5).

**The script heap, in the fork** (R49 "Next")
- Shared atoms: built in R51 ([Round 51](#round-51-shared-atoms)), phases 1 to 3, with the embedder's declared keys.
- The inline-cache table: trimmed and compacted at a collection since R50 (80a91e10).
- A compacting value heap: the arenas give back only their ends.
- Gate status: every test-apps process under it since R52 (the largest 62 MB of 73, with the half-second idle
  release). It rises as the apps add code and data.

**Memory beyond the script heap**
- The process was 480 MB on glibc and 603 falling to 502 MB on jemalloc the one time it was read (R37), against a
  script heap of 62 MB at most. What the rest is has not been looked at: [Profiling3](Profiling3.md) M7.
- jemalloc's decay (`_RJEM_MALLOC_CONF`) has not been tuned for a quiet instance (R37).

**Engine differences, settled** (2026-11-09; Oxide "Engine differences under `oxide`", "Phase 3: engine differences")

The rule was the standard where there is one. "QuickJS" in these notes was rquickjs, which bundled quickjs-ng 0.16;
Bellard's QuickJS 2026-06-04, the fork's reference, already behaved as oxide does on several of them.
- `using` and `await using`: explicit resource management (ES2026) is in the fork (FORK-EXPLICIT-RESOURCE-MANAGEMENT-001),
  with native `SuppressedError`, `DisposableStack`, `AsyncDisposableStack` and the dispose symbols; the bindings' JavaScript
  versions are gone.
- `performance`: the fork's `Context::install_performance`, as High Resolution Time has it (`now()` in fractional
  milliseconds on a monotonic clock, `timeOrigin` an epoch time).
- `Error.captureStackTrace` and `Error.stackTraceLimit`: in the fork (FORK-CAPTURE-STACK-TRACE-001), as V8 and
  quickjs-ng have them; errors keep their own `stack`, as Bellard's do, so there is no `Error.prototype.stack` accessor.
- Stack columns and native messages: kept. Bellard's QuickJS gives oxide's (`(`, `stack overflow`, `cannot convert to
  object`); quickjs-ng's are V8's, and no standard covers either.
- A decimal re-read after a releasing flush is the same object while the stack holds an equal decimal (the bindings
  track it as a list).
- The frozen helper globals win over a context value of the same name: kept. It fails safe for `UserSecurityChecks` and
  is close to Groovy, which resolves a class name before a binding variable.

**Allocation and CPU leftovers** ([Profiling3](Profiling3.md) §5 lists what a service call, a find and a row still
do on every call, with the lines: W1 to W17)
- `Value::clone` and the maps built to call services (R15).
- Allocation (12 to 13%) and `memcmp` (3 to 3.7%) in a service call (R27, R29, R30).
- OpenTelemetry attribute-set hashing and `IndexMap`s of values (R29).
- Span names copied, and the transaction span's attributes set per call (R21).
- A data-feed touch keeps two `String`s; an entity with cached finds keeps the values written (Further work 9).

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
plans; the regexp's lazy loops and failed-run skip; the compacting heap. Shared atoms were built in R51
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

**Tier 3: later** ([Profiling3](Profiling3.md) orders what comes next; its §6 takes up three of these)
- **PGO builds:** zipp B114, −13% overall, −10 to −30% where the interpreter dominates (Zipp 12). Cheap enough to
  try as one experiment now (Profiling3 S5e).
- **The dispatch loop,** after a new qvt profile, within the loop size gate (Zipp 13):
  - fused instruction pairs (−5%; zipp's triples lost 9 to 20% elsewhere);
  - frameless leaf calls (B117);
  - `typeof x === "lit"` as one instruction (B54);
  - `x == null` with no heap read.
- **`using` and `await using`,** with zipp's grammar and DisposeResources as the reference (Zipp 14).
- **A generational GC:** the fork's `docs/planned.md` §6, with zipp's NURSERY_DESIGN.md to read (Zipp 15).
- **A JIT,** a distant option: deadlines by step loans per block, `MAP_JIT` on macOS and none on iOS (Zipp 16). Not
  by these numbers: the interpreter's loop is 1% of a transaction's samples and 12 to 24% of a list render's
  (Profiling3 §3, §8).
- **The database side:**
  - binary parameters;
  - the per-thread definition memo;
  - table creation in parallel at a first start;
  - a template cluster;
  - the transaction cache on by default for some kinds of service (Further work, R9, TX §Later rounds; a test now,
    Profiling3 S1).
- **More native actions:** R52's plan covers bodies of only simple `<set>`s; `<if>`, `<iterate>` and
  `entity-find-one` over the same plan would take loop#Actions and the screens' actions off the engine. Profiling3
  S3c gives it a shape: one parsed form of a body, the script and a native executor as its two back ends.

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
| `UrlInstance` getters and `sri.isAnchorLink` as leaves (no lend) | instructions flat on every screen (×0.998 to ×1.011) | R53 |
| A link's `parameter-map` evaluated in the unit and passed to `makeUrlByType` | flat (×0.993 to ×1.003): the nested script costs what the inline code does | R53 |

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
- A service worker idle half a second (a second before R52), with two others idle, lets its runtime go; its next
  script starts one.
- A runtime its heap leaves far past its live bytes is dropped and started fresh (R44, R45).
- Inert modules are kept for an outermost run (`script_module_cache`).
- A runtime's start texts are compiled by the process's first runtime and loaded from its code images by later ones
  (R50).
- Builtin atoms, the atoms of code images, and the keys mr-moqui declares (a runtime's start, the entity model's names)
  are the process's, in one shared table that is never collected; a runtime keeps its own atoms for other texts, keys
  from run-time data among them (R51).
- `ec.entity.find` and `EntityFind.condition` run without lending the engine, except for a definition made from rows
  (R50).
- A list or map read by bare name keeps its host type. A decimal read twice is the same object until a releasing flush.
- An actions body of only simple `<set>`s runs without the engine, and its values keep their host types: a Timestamp,
  Date or Time stays one (the script made it text), an F64 or I64 stays one. `echo#Data` returns `timestampOut` as a
  Timestamp, as Java does (R52; the question open since R11).
- `<set>`'s `default-value` applies where Groovy's `?:` does (null, "", 0, false, an empty list or map), and
  `set-if-empty="false"` skips what `ObjectUtilities.isEmpty` calls empty (null, "", an empty list or map) (R52).
- A chained `EntityFind` call returns the same find, which can run again; "entity find was already used" is gone, and a
  find is refused only while it runs (R52).

- **`<set value>` and the other GString texts are not localized (R55, Profiling3 W14, D1).** As Java's template writes
  them as GStrings, a `set`'s value and default-value, a field-map value, a compare value and a name are expanded with
  `ec.resource.expandNoL10n`, or are a literal when there is nothing to expand; before, they went through
  `ec.resource.expand`, which localizes, so a value equal to a localized message's original came out translated in a
  larger body but not in a body of only sets (the native plan). Messages, asserts and log text are still localized.
- **An in-map of the context passes only the declared in-parameters (R55, W2)** when the service validates its input,
  has no pre-validate rule, is not entity-auto, and the call is not multi: what the service sees is as before, since
  validation dropped the rest.
- **A data feed probe whose savepoint rollback fails is reported as a failure before the feed (R54).** Its savepoint
  goes out behind what is unanswered, so the two cannot be told apart; the transaction does not commit either way.

## 8. Known flaky tests

- `service::coverage::tests::calls_are_counted_only_between_enable_and_finish`: a process-wide recorder, races with
  tests in parallel (R29).
- moqui-core `tarpit_locks_and_check_views_are_read_once_until_a_lock_is_written`: a process-static authorization
  generation (R22, R25; adjusted in R25 and R28).
- moqui-host's XMPP walk (`xmpp_clients_sign_in_…`): "not in time" under a full run's load, passes alone (R29, Phase 4).
- moqui-host's `rtc::cascade::a_call_on_two_nodes_carries_media_both_ways_…`: "the leg node did not start" (a child
  server past its start timeout) under a full run's load, passes alone (R60).
- moqui-git `mirror_fetches_and_opted_in_writes_raise_events` and `the_tree_refs_compare_and_merge_methods_through_services`:
  timing under a full run, pass alone (R48, R49).
- mr-boss `WorkCenters` load of a released operation: intermittent (R22, R27, R28, R49).
- moqui-host `devaide::the_index_follows_reloads_and_extensions`: a restart re-read 9 files in a full
  `test-framework` run, passes alone (R53).
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
- **[Profiling3](Profiling3.md):** the second reading of these numbers (2026-11-10), the instruments still missing,
  and the next work in order. It adds no measurement of its own.
