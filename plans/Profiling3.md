# Performance: a second reading, and where to work next

Status 2026-11-10: M1 to M4 and M6 are built (Profiling2 Round 54), and W1, W2, W4 to W10 and W12 to W17 (Round 55);
W3 is not built and W11 goes with S4a. S1's first part (Round 57), S2 (Round 58, in mr-moqui without a fork change)
and S3a and S3b (Round 59) are built; S4a, S5a and S5b are sized and not built. Round 60 ran the load flows, set the
pools (D6) and turned the transaction cache on by default (D3). The rest is the plan. It reads the numbers of [Profiling2](Profiling2.md), the
archived rounds and the bench repository again with the code beside them, and proposes what to measure and what to
change next. No benchmark was run for it. Two figures were computed from files already kept (the Mantle census and
the action bodies under `runtime/`, both in [§10](#10-sources)); every other number cites its round. A `file:line` is
what the code said on 2026-11-10 (mr-moqui c3e861f8, the fork b7e34e04); paths with no crate are under
`crates/moqui-core/src/`. The user's decisions of the same day are in [§9](#9-decisions-and-open-questions).

Profiling2 stays the record: the headline numbers, how to measure, each round, and what was dropped. This file adds a
reading of the numbers (§2), a theory of what suits this architecture (§3), the instruments that are missing (§4), the
small certain work (§5, items W*n*), the larger work (§6, items S*n*) and an order (§7). Its targets are to be proved;
none is a result.

In short:
- A business transaction's time is its database round trips, and two thirds of them have never been classified.
- The script interpreter is 1% of a transaction's samples and 12 to 24% of a list render's. A JIT does not fit.
- What costs is generic work done again on every call: names resolved, maps copied, texts hashed, locks taken. A
  service of four sets spends 95% of its instructions outside the sets.
- The speed rows are a few lines long and run on one thread, so they do not show costs that grow with a body, a
  context or a thread count. Real service bodies are 5 KB at the ninetieth percentile.
- The next large gains are in compiling a definition further before it runs (plans and templates the host fills),
  and in a write-back cache for context names that the engine already has the fast path for.

## 1. Terms

- **A crossing:** one call from script to a host method, with its argument and result conversions.
- **A lend:** the engine handed to a host method that may run a script itself (Profiling2 §3, F3a).
- **The frame, the census:** a transaction's cache frame and the per-transaction line of
  `MOQUI_TX_CACHE_CENSUS_FILE` (Profiling2 §2).
- **A body:** the XML of one `actions`, `condition` or similar element, the unit a script is generated from.

## 2. A reading of the numbers

### The cost ladder

What one of each costs now, on the Linux machine, one thread:

| One of these | Costs | Proof |
|---|---|---|
| a step of a script loop over a local | 9 ns | R52: 1000 adds to a local 8.77 µs |
| a host call, the fork's own floor | about 30 ns, 560 instructions | the fork's `docs/embedding.md` |
| a bare name read or written | about 190 ns | R52: 1000 adds to a bare name 380 µs, a read and a write each |
| a host getter or small host method | 0.24 to 0.33 µs, 2 to 5 allocations | R52: 100 cache gets 24.2 µs, 100 `nowTimestamp` 32.6 µs |
| a host call in a list render, on average | 0.56 µs in its body and 0.2 µs converting | R53, `work/r53/profile-h4.jsonl` |
| an expression evaluated from Rust | 0.44 to 0.76 µs | R52: value `a`, a condition |
| an entity cache hit in Rust | 0.82 µs, 12 allocations | R4 |
| the same find from XML actions | 4.45 µs, 44 allocations | R52: entity-find-one, cached |
| a service call with an empty body | 8.0 µs, 84 allocations | R52: noop#Inline |
| a `service-call` from actions, on top of the service | about 9 µs | R52: call#Nested 17.0 µs, set#One 7.9 |
| a statement to a local Postgres | 20 µs on the socket, 29 to 34 over TCP | R4, R5 |
| a commit on a disk | 250 to 300 µs | R5 |
| a statement to a Postgres 200 µs away | about 300 µs | TX §The commit batch |
| a script runtime's start | 1.42 ms | R52 |

A service call is the price of 900 loop steps or 40 bare names. A local statement is two and a half service calls,
and a remote one 37.

### A business transaction

The nine Mantle operations against a Postgres 200 µs away, from run `ox1` of R48's acceptance (medians over each
operation's frames in the census file). "Not classified" is the round trips less the reads the frame counted
(`db_reads`) and one for the commit batch:

| Operation | ms | Round trips | Reads counted | Not classified | µs a round trip |
|---|---:|---:|---:|---:|---:|
| `place#Order` | 21.4 | 66 | 21 | 44 | 324 |
| `approve#Order` | 16.1 | 54 | 17.5 | 35.5 | 299 |
| `add#OrderProductQuantity` | 16.4 | 52 | 17 | 34 | 316 |
| `change#InvoiceState` | 24.4 | 80 | 31 | 48 | 304 |
| `create#Payment` | 24.0 | 77 | 20 | 56 | 312 |
| `apply#Settlement` | 48.6 | 120 | 76 | 43 | 405 |
| `pack#Shipment` | 43.4 | 117 | 45 | 71 | 371 |
| `get#PaymentFinanceDetail` | 12.9 | 27.5 | 26.5 | 0 | 470 |
| `approve#Payable` | 28.7 | 84 | 44 | 39 | 342 |

- **The time is the round trips.** An operation costs 300 to 470 µs for each, whatever it does between them. Across
  the whole run the frames counted 6,157 reads in 67,474 round trips.
- **Most of an order operation's round trips have no name.** The frame counts the reads it could have answered. The
  rest are candidates only: finds that go round the frame (`for-update` finds always do, TX D4; paged lists, counts),
  shared-cache misses, sequence blocks, nested `force-new` frames, cold statements that prepare. Nothing recorded
  says which. This is the largest part of a transaction's time, and it is unexamined.
- **The application's CPU has not been measured apart.** What is left after the round trips at 0.3 ms each is 1 to
  2 ms an operation, an estimate. R47's profile is the only one: the script layers 18% of the server's samples,
  `run_at` 1.1%, and a third of all samples Argon2 from the specs' logins.
- **On one node both matter.** Over a local socket place#Order's 66 statements are 1.3 to 2.6 ms, the same order as
  that CPU estimate. With the database on another node, only the round trips matter.

### A service call

`echo#Data` is 8.4 µs, 94 allocations and 182 thousand instructions. Its body, four sets, is 5 to 9 thousand
(quickjs-oxide-bench `rounds/r52/m1`). So 95% of the call is the call: the name made canonical, the definition found,
six SECA lookups, the parameters cleaned, authorization with its tarpit check, an artifact frame with telemetry, a
transaction, a context frame, the parameters copied into it, the actions found by their XML text, the out-parameters
listed twice, and the artifact's statistics recorded under a process-wide lock (§5 has the lines).

From a script there is more. The generated chain
`ec.service.sync().name(..).parameters(ec.context).parameter(..).call()` is five or more crossings, each a lend, and
`.parameters(ec.context)` copies every name the caller can see before validation drops the undeclared ones.

### A list render

Tools Entities in html, 1,482 rows of three links, at the end of R53:

- 92.5 ms and 1,248 M instructions a request: about 840 thousand instructions and 62 µs a row.
- About 65 host calls a row (96,571 a render in the last render profile, `profile-h4`).
- By that profile (137 ms with it on): 43% units, the interpreter between host calls; 39% host method bodies; 14%
  converting arguments and results.
- By `perf` at the round's base (`stacks-B-html.txt`): `run_at` 17% of self time, the allocator 11%, `memmove` and
  `memcmp` 7%, SipHash 2% and 10% with its callers, collection 3 to 4%, `Heap::shape_lookup` 2.5% (property reads
  with no inline cache). Telemetry spans were 10% then and went in the round.
- There is no `perf` profile of the build at the end of the round. The profiles after the first changes
  (`stacks2-B-*.txt`: `run_at` 19% in html, 24% in qvt, 12% in tui) ran with the render profile on, and its
  bookkeeping is 9 to 17% of them with what it calls.

Writing a row from a prepared template in Rust would cost thousands of instructions, not hundreds of thousands (an
estimate from the work a row is: a few field values formatted and escaped, three URLs with their parameters). Against
Moqui Java, Cache List is 9.0 ms to 7.6 ms warm (R53): the two are level because both do the same thing, a generic
widget runtime interpreted for every row.

A small page is its data. The Dashboard's 6.5 ms holds two service calls of 2.1 ms together (`profile-base.jsonl`:
1,136 host calls, `ec.service.sync().call` 2.09 ms of 7.08); the shell around it is not a fixed cost worth a lane.

### What the speed rows do not show

- **Body size.** Service bodies are 0.7 KB at the median, 2.2 KB at the 75th percentile, 5.2 KB at the 90th, 15.7 KB
  at the 99th and 82.5 KB at the most (4,516 bodies under `runtime/`). The services of `js_speed` are one to four
  lines. Every actions run finds its script by the whole XML text: SipHash of it and a compare, under one
  process-wide mutex (`script/actions.rs:179`, `script/bounded.rs:54`), then a hash and a compare of the generated
  text (`script/engine.rs:142`, `script/oxide/compiled.rs:164`). Not measured in place. A std `HashMap` lookup by a
  text key, timed alone on this machine, is 0.22 µs for 1 KB, 0.80 µs for 5 KB and 2.5 µs for 16 KB: a tenth of a
  service call for a body at the 90th percentile, and all of it inside the lock.
- **Context size.** A service call's cost grows with what its caller holds. R52's rows show it once: 10 noop calls
  are 87 µs, and 163 µs with a 100-map list in the context that no call uses.
- **The cache's bound.** The generated-script cache keeps 4,096 texts (`script/actions.rs:35`) and drops the least
  used quarter when full. The tree holds 8,743 distinct bodies. A process that uses more than the cap generates again;
  nothing counts how often.
- **Threads.** Only `entity_speed` runs on more than one (R7: a cached find scales to two threads and stops). The
  per-call path takes process-wide locks: the actions cache above; `record_artifact` on every artifact pop
  (`telemetry/mod.rs:1070`); the tarpit map where a rule exists for the artifact type (`user/authz.rs:637`).
- **Workers.** Both pools wake every idle worker for each job (`service/request_pool.rs:142`,
  `service/pool.rs:596`). Each wake restarts the worker's idle wait (`service/pool.rs:1079`), so under steady light
  traffic no worker reaches the half second that releases its runtime, and work spreads over all 25 (read in the
  code, not measured).

### Memory

The script heap has had six rounds and is 62 MB at most in a test-apps process (R52). The process was 480 MB on
glibc, and 603 falling to 502 MB on jemalloc, the one time it was read (R37). What the other 400 MB is has not been
looked at.

## 3. The theory

**A JIT does not fit.** The interpreter's loop is 1.1% of a transaction's samples and 12 to 24% of a list render's.
A baseline JIT removes dispatch, part of that share. It cannot remove a host call, a conversion, a lookup by name or a
copy, which are the rest. It would also need `unsafe` in an engine that forbids it, a backend per architecture, and
warm code for runtimes that are let go after half a second idle. The plain JavaScript rows already beat QuickJS
(Profiling2 §3).

**The generators are ours.** A JIT learns at run time what a program keeps doing, then specializes. Here the program
is generated from a definition, and the definition already says what a JIT would have to learn: which widget a cell
is, which of its attributes are constants, which names a body reads, which service a call names. So the place to
specialize is where a definition is compiled. Today's generators are macro expansions: they emit a call to a generic
runtime function for each element, and that function finds out the constants again on every row and every call. R53's
`displayOf` and `linkOf` memos, and R12's "attributes read once", are this idea applied one case at a time.

**Three patterns** cover most of what follows:
1. **Resolve once per definition.** A definition handle, a prepared script, a rule list, a parameter plan and a
   statistics slot are found by name or by text on every call now. Each belongs on the definition.
2. **The host drives plain work, and the script is called for expressions.** Today the script drives and calls the
   host for everything: 65 crossings for a row, five for a service call. R52's native `<set>` is the proof of the
   other way round: ×0.16 to ×0.20. The context stack is the host's already, so a host-driven step and a script
   expression see the same names.
3. **A write-back cache instead of a crossing per access.** Tracked lists and maps are one (R11). Scalars by bare
   name, entity fields and output writes still cross every time.

**The order of what matters.** For a transaction: statements, then the service call's own work, then names and
crossings in the body, then the interpreter. For a render: crossings per row, then the work in the host bodies they
reach (URLs, row frames), then the macro runtime's script, then the interpreter.

**Memory and speed meet at the worker.** A script runtime belongs to a thread, and a thread waits through its
round trips. With the default eight request workers and a database 200 µs away, place#Order holds a worker 21 ms, so
the pool completes at most about 380 a second however many cores there are. More workers cost a runtime each (1.5 to
5 MB, R49). Fewer round trips is the lever that helps both; the pool sizes themselves are to be tuned (§9 D6).

## 4. Measure first

Each of these is small, and each decides an item of §6. M1 and M2 come first: they are the two with nothing recorded.

- **M1. An operation profile.** `screen/profile.rs` opens its spans only inside a screen render
  (`screen/render.rs:690`); the span sites in the binder and in `run_actions_at` record nothing for a service call.
  Open it for any outermost service call or transaction too (`MOQUI_OP_PROFILE`), and add counts of service calls,
  entity operations by kind, bare-name reads and writes, and the thread's CPU time beside the wall time. It answers
  what an operation's CPU is made of, which R47 could not.
- **M2. Statement shapes in the census.** For each transaction, its statements by shape (a hash of the SQL text and
  its first words), each with whether it was waited for, and its kind: a key read, a list, a count, a view, a
  locking read, a sequence, a write batch, a commit. It names the round trips of §2's table, and it counts two
  things for §6 at no further cost: the same shape run again with other parameters (a find in a loop), and reads in
  a row with nothing between them.
- **M3. Rows the size of the work.**
  - `mantle_speed`: the nine operations in process on the embedded Postgres, with the harness's columns
    (allocations, statements, round trips) and CPU time. This is Stage B of
    [MantleUSL-Execution-Benchmark-Plan](MantleUSL-Execution-Benchmark-Plan.md), possible now that mr-mantle runs.
  - `js_speed` rows shaped like a real service: a 5 KB body, 30 names in the context, a 50-map list the caller
    holds, a service call inside a loop.
- **M4. Thread rows.** `speed::Rows::run_threads` (R7) on 1, 2, 4, 8 and 16 threads for noop#Inline, a cached find
  from actions, a find in the database from a script, and List html. Then `perf c2c` on what stops scaling.
  [LoadRunner](archive/LoadRunner.md) does the same for whole flows over HTTP once built.
- **M5. A clean render profile.** `perf` of the current build for Entities and Cache Elements in html and qvt, the
  render profile off, split by layer with R33's `layers.py`. The output now streams (0a1f1a41), so this is also the
  new base.
- **M6. Engine counters through the runtime.** Inline-cache hits and misses by kind, and how many property sites
  fall past a function's 255th (the fork counts them for `qjs --ic-stats`); collections and their time per
  outermost run; scripts generated against distinct bodies seen.
- **M7. A process memory census.** jemalloc's own statistics (allocated, active, resident, retained) and a heap
  profile at idle and after 50 page loads, by subsystem: definitions, caches, code images and shared atoms, the
  script heaps, thread stacks, search.
- **M8. Moqui Java on the same flows.** LoadRunner's order to cash on both servers, with M1's and M2's counts on
  this side, so a difference can be put on round trips or on CPU.

## 5. Weeding

Read in the code, each certain and small. They are the "resolve once per definition" pattern and a few copies. Proved
on the rows' allocations and instructions, and on M3's rows. Targets: noop#Inline 8.0 µs and 84 allocations to under
4 µs and 40; call#Nested 17 µs to under 9; a cached find from actions 4.45 µs to under 2.5.

| # | What | Where |
|---|---|---|
| W1 | The prepared actions (script and plan) kept on the definition that owns the body, so a run takes no lock, hashes no XML and is not bound by the 4,096 cap; the script's kept hash passed on, no file name formatted per run, a compiled hit by hash and the text's address | `script/actions.rs:77`, `179`; `script/engine.rs:142`; `script/oxide/compiled.rs:164` |
| W2 | `in-map` of the context: read the declared in-parameters from the stack, where validation would drop the rest (`validate::clean` keeps only the declared names when the service validates its input) | `script/host/service.rs:222`; `script/oxide/bind.rs:746`, `964`; `moqui-lib/src/context_stack.rs:302`; `service/validate.rs:95` |
| W3 | The cleaned parameters moved into the service's frame, not cloned one by one | `service/call.rs:1382`, `1410` |
| W4 | The expanded out-parameters kept on the definition as the in-parameters are; they are expanded twice a call when a service has `auto-parameters` | `service/validate.rs:380`; `service/call.rs:1394`, `1434` |
| W5 | A definition's SECA rules by phase, found once: each of a call's six lookups allocates the name without its `#` | `service/seca.rs:109`, `152` |
| W6 | Artifact statistics without the process-wide lock: a slot on the definition, or a table per thread merged when read. Today each pop hashes three strings with SipHash and locks | `telemetry/mod.rs:1070`; `context/artifact.rs:118`, `299` |
| W7 | Authorization's per-check strings: the action name, the tarpit key and its map lock | `user/authz.rs:637` |
| W8 | A find holds its entity's definition, not its name: the name is resolved again in `find`, `one_from_cache`, `cached_list`, `has_empty_pk`, the artifact push and the plan. A map condition builds the entity's field names as a set on every call | `moqui-entity/src/entity/find.rs:52`, `384`, `400`, `525`, `799`; `script/host/entities.rs:308`, `512` |
| W9 | A write's SQL key built as a string per write; each matching entity ECA rule cloned per write | `moqui-entity/src/entity/value.rs:1470`; `moqui-entity/src/entity/load_feed/eca.rs:64` |
| W10 | foldhash for the context stack's frames and `Value::Map`, which hash with SipHash once per frame on a bare read, and for the run's tracked maps, the generated-actions cache and the telemetry keys. R29 moved the internal maps and these stayed. Decided (§9 D2); then the rest of our own maps on std's default hasher, the hot paths first | `moqui-lib/src/context_stack.rs:32`, `168`; `moqui-lib/src/object.rs:59`; `script/oxide/tracked.rs:62`; `script/bounded.rs:21`; `telemetry/mod.rs:1070` |
| W11 | A form-list row is copied out of the list, copied again for `_formMap`, and the list's `info` object is converted on every `startFormListRow` | `screen/sri/forms.rs:214`, `596`; `script/host/sri.rs:242` |
| W12 | A screen unit found without formatting a key and locking, per section per render | `screen/generate.rs:254` |
| W13 | Wake one worker, the most recently idle, in both pools. Work then stays on few threads, their runtimes stay warm, and the others' idle release can fire | `service/request_pool.rs:142`; `service/pool.rs:596`, `671`, `731`, `1079` |
| W14 | A `<set value>` with no `${` as a literal, and one with it through `ec.resource.expandNoL10n`: the macro always emits `ec.resource.expand(..)`, a crossing that localizes first, where Java's is a GString that does not (§9 D1) | `runtime/components/base/framework/template/XmlActions.js:46`; `script/oxide/resource.rs:56` |
| W15 | An in-script `expression` or `condition` formats and hashes its text on each call; `localize` formats a cache key on each call | `script/oxide/resource.rs:242`; `l10n.rs:173` |
| W16 | `$esc` of the fancy templates: a test before the replace, as `e` of ScreenHtml.js has | `script/template.rs:638` |
| W17 | `isAnchorLink` copies the whole `UrlInstance` for each link | `script/host/urls.rs:127` |

W1, W6 and W13 are also the first suspects for M4.

## 6. Structural work

Larger, each with a design to settle and a gate from §4. The effects are targets.

### S1. Round trips

After M2 names them. The candidates, from the general to the particular:

- **The transaction cache as the default: a test (§9 D3).** It is on for about 60 services now. A setting beside
  `screen_tx_cache` makes a service's transaction hold when its definition says nothing; a `cache`, `force-cache` or
  `no-tx-cache` on the definition or the call stays as written. Turn it on and see:
  - **The suites.** Census mode (`moqui.tx_cache.everywhere`, dev and test only) is the nearest run so far: 3,062 of
    3,067 features passed with every frame holding (R22), and Census 9 had 42 early flushes in 152,290 transactions
    (R26). The test is whether the early flush and the switch to read-through are enough when holding is the rule
    and not a census.
  - **Where it only costs.** Holding has its own price: allocations (S4 29k to 48k a call, TX §Reads), and over a
    local socket some calls were slower with it until R15 and R16. A small service with one statement and no read
    of its own writes gains nothing. `tx_cache_speed`, `mantle_speed` and the nine operations at 200 µs, off against
    on, with M1's CPU and M2's round trips per service, show where.
  - **The rule for a service with no setting of its own,** chosen from what that shows. Candidates: by the frame's
    own counts for the service over its last calls (reads answered from memory, writes sent as a batch: when there
    are none, read through the next time); by the body (one entity operation and no service call holds nothing
    worth holding); by the database's distance (the round trip measured when a connection opens).
  - The risks stay TX's Decisions 1 and 2: a held frame does not see other commits, and locks wait for the commit.
- **Reference data through the entity cache.** Which of the reads are of rows that seldom change (enumerations,
  statuses, settings, a store's configuration)? M2 by entity shows them.
- **A find in a loop.** The audit counted 867 entity and service operations under XML loops in 293 Mantle services.
  When a frame holds, and a find by key misses for a key that came from a row of a list the frame read, read that key
  for the list's other rows in the same statement (`= ANY`), into the frame's rows by key (TX D2). M2's count of a
  shape run again says what it would save before anything is built.
- **Reads sent together.** The wire client already sends writes without waiting. A find's row could stay on the wire
  until the script first reads the value, so two finds in a row wait once. A null check is a read, so this pays only
  where M2 shows runs of reads; errors would move to the first read. A candidate, not a plan.
- **A locking read and its write as one statement,** where the row read `for-update` is then updated.

Gate: `mantle_speed`'s round-trip column, and the nine operations at 200 µs. A third fewer round trips is a third
less time at any distance.

### S2. Context names as global lexical bindings

A bare name costs 190 ns each way: the exotic hook, the name as text, SipHash for each context frame, and a
conversion (`script/oxide/names.rs:86`, `script/oxide/tracked.rs:107`, `179`). Generated actions are made of bare
names.

The engine already has the fast path. `GetGlobal` and `SetGlobal` look first in the global lexical environment, and
a site that found a binding there is cached: one generation compare and a load, or a store (the fork's
`core/src/vm/interp.rs:1504`, `1562`, and the inline arms at 2389). Only a name found nowhere reaches the exotic
prototype that reads the context.

So keep the context's names there, as a write-back cache:
- **A first read** goes to the hook as now, which also makes the name a binding. Later reads and writes are cached
  sites and never leave the engine.
- **A flush** (before a host method that reads the stack, before a nested run, at the end: the same points tracked
  values flush at, `host::Method.flush`) writes to the stack each binding whose value differs from the one last
  synced. A value is eight bytes, so finding the changed ones is a compare per binding, with no write barrier.
  Lists and maps stay under today's tracked compare.
- **After the host may have changed the stack** (a method that releases tracked values, a frame pushed or popped, a
  nested run's return) the bindings are emptied and the generation moves. An empty binding of this kind asks the
  hook again, where an ordinary one in its dead zone throws.
- **Never worse than today by much:** the worst body reads one name between two releasing host calls, and pays
  today's read plus a compare.

To settle in the design: a nested run shares the realm, so it enters with the bindings emptied and its caller's are
filled again on demand; a name the global object owns must keep winning, so the hook makes no binding for one; and
one generation counter serves real globals too, so emptying also sends `ec` and `Object` once through the slow
lookup. If that shows, embedder bindings get their own counter. It is a fork addition; whether upstream would take it
does not decide it (§9 D4).

Rows and targets: 1000 adds to a bare name 380 µs to under 40; iterate 100 context maps with if and set 233 µs to
under 120; loop#Actions 307 µs to under 150 (loop#Script is 53). Gate: M1's count of bare-name reads and writes in
the Mantle operations says how much of an operation this is.

### S3. One crossing for a generated action, then action plans

- **S3a. `service-call` as one crossing.** A hidden host function takes the service (a definition handle kept by
  the call site after its first run), the options as bits and the explicit parameters, reads the declared
  in-parameters itself when the in-map is the context (W2), and puts the results where the out-map says. Today the
  result goes host to script to host through `Object.assign`
  (`runtime/components/base/framework/template/XmlActions.js:336`). The builder stays for scripts people write.
- **S3b. `entity-find-one` and `entity-find` as one crossing,** with the entity's handle, the field map by name and
  the flags, in place of `find().condition().one()`. Target: a cached find from actions 4.45 µs to 1.5 (Rust's is
  0.82).
- **S3c. Action plans.** R52's `SetPlan` (`script/actions_set.rs`) grown into one parsed form of a body: set, if,
  iterate, the finds, service-call, return, messages, and script. An expression is *simple* (a literal, a name, a
  dotted path, a comparison with a literal, a null check, `!`, `&&`, `||`), evaluated in Rust under rules shown
  equal to the script's, or it is *script*, a compiled function entered at 0.44 to 0.76 µs. The same form feeds
  both the script generator and a native executor, and a body takes the native one when its script expressions are
  few beside its host operations. This is the shared action IR the Mantle audit asks for, and the shape of
  Profiling2 §5's "more native actions".

  Its risk is the audit's: a plan that enters the engine for every condition loses to the whole body as one script.
  Hence the choice per body, made when the body is prepared, and the rows: iterate with if and set, loop#Actions,
  `mantle_speed`.

S2 comes before S3c. It is smaller, it helps hand-written scripts too, and it shows how much is left for S3c to win.

### S4. Form-list rows as templates

Now each cell's unit code calls a runtime function (`display`, `link`) that asks the host for the field's texts, the
URL, the disabled flag and the icon, builds strings, and writes: 65 crossings and 840 thousand instructions a row.

- **S4a. One crossing a row.** At `openFormList` the host prepares what is the same for every row of a column: the
  resolved path, the permission, whether it is an anchor, the url-type, the constant parameters, and which fields
  the others come from. A row is then one call that returns every planned cell's texts and URLs. `makeUrlByType` is
  3.7 µs a link today, most of it found again each row (`screen/urls.rs:375`, `467`, `785`, `981`).
- **S4b. The row as a template with typed holes.** When a unit is generated, the mode's macros emit for the list a
  row template in place of row code: literal text; a field's value by kind (text, plain, class) with its format and
  its escape; a URL by its plan; a condition on a field; and script parts for what is an expression. Rust fills it
  for each row and appends to the output, entering the engine only for a script part. The markup stays defined once,
  in ScreenHtml.js, ScreenQvt.js and ScreenTui.js, which run in a template mode at generation; Rust knows holes, not
  HTML. Escaping happens as a hole is written, with no string between.
- The same serves csv, text and xlsx exports of long lists, and later a form-single's fields (Single html is 28 µs
  a field).

- **Whitespace becomes a choice (§9 D5).** A template's literal parts can be written as the macros gave them or with
  the whitespace between tags stripped, by mode or as an option. The output need not be byte-equal to today's.

Gate: the outputs the same by `compare_bodies.py` and `JS_SPEED_DUMP`, apart from whitespace where it is not content:
html and qvt compared with the whitespace between tags normalized; text, csv and anything preformatted byte for byte.
Targets: Entities html 92 ms to 30 with S4a and to 15 with S4b; List html, 100 rows 8.3 ms to under 2.

### S5. The engine

- **S5a. The host call's floor.** 560 instructions with no argument, in the fork's own account: a clone of the
  function's body `Rc`, a `Weak` upgrade, a clone of the shared state, a site pushed for backtraces, root marks, the
  released-handle drain, a poison guard, the sampler check. A host function registered as a leaf (no re-entry, no
  handles made) needs few of them. Target 200. Rows: 100 cache gets, 100 `nowTimestamp`.
- **S5b. A string argument read where it lies.** `sri.write` copies its text into an owned `String` and then into
  the body, 50 thousand times an Entities render. The fork already shows a key's text in place
  (`visit_plain_own_text`); the same for an argument lets a write transcode straight into the output.
- **S5c. Inline-cache coverage.** Only a function's first 255 property sites are cached (the fork's
  `core/src/vm/ic.rs:68`), and generated bodies and screen units are long functions. It may be part of what
  `shape_lookup`'s 2.5% of a render is; nothing says yet. M6 counts it; then either wider site numbers in the fork
  or shorter generated functions.
- **S5d. A changed mark for tracked objects.** A flush compares a tracked list with the stack's in place (R34, R52).
  A mark the engine's store paths set on an object the embedder asked to watch would let a flush skip an untouched
  one outright. Row: 10 noop service calls with `itemList` tracked, 163 µs against 87.
- **S5e. A profile-guided build.** One experiment: the release binary built with a profile from `js_speed` and
  `screen_bench.py`, timed on both, with the build's cost. zipp recorded −13%; R52 found the interpreter rows move
  10% with where `run_at` lands, and a profile fixes the layout. Profiling2 has it in Tier 3; it is cheap enough to
  try now.
- **S5f. Entity fields.** A field read is a hook, a lock, a clone and a new string each time
  (`script/oxide/names.rs:142`). The same write-back idea fits: the wrapper keeps the fields it has read as its own
  properties. Only if M1 shows field reads matter.
- **S5g. String payloads off malloc.** Every string that is not from a table is a `malloc` (the fork's
  `heap/string.rs`), and the allocator is 9 to 13% of a render. R46 measured inline short strings at 2% and dropped
  them. Payloads in a byte arena could move at a collection, since only the string's record names them. Only if M5
  still shows the allocator near 10%.
- **Collection** is 3 to 5% of a render and is not a target. The fork's budget floor (`min_heap`, 1 MiB) is a knob to
  try under load.

### S6. Threads and memory

- Whatever M4 shows after W1, W6 and W13: R7's leftover (the slots lock's reader count, a cache entry's atomics, a
  per-thread memo of definition lookups) is the known next one.
- **The pools' defaults** (`request-pool-max` 8, `worker-pool-max` 25; `service/request_pool.rs:30`,
  `service/pool.rs:43`), from M4's rows and then LoadRunner's flows: changed when a test says so, and tuned as load
  testing goes on (§9 D6). A larger pool is cheaper once W13 keeps the idle workers' runtimes released.
- Whatever M7 shows. The process, not the script heap, is where a quiet instance's memory is.
- **A runtime's start** is 1.42 ms. The fork's planned heap snapshot (`docs/planned.md` item 3) would make it a
  copy, and a cheaper start lets an idle runtime go sooner. After W13, which lowers the count of warm runtimes first.

## 7. Order and gates

1. **Instruments:** M1 and M2, then M3 and M4. M5 to M8 alongside. One round.
2. **Weeding:** W1 to W17, each on its rows; then M3's and M4's rows again. One or two rounds.
3. **S1:** the transaction cache's default as a test (§9 D3), which needs only M1 to M3; the rest with M2's table in
   hand.
4. **S2,** a fork branch and the bindings on it, by the fork's Sweep and `gates.sh` as any fork item.
5. **S3a, S3b and S4a:** the coarse crossings.
6. **S3c and S4b,** sized by what the steps before left.

S5e can run at any time. The rest of S5 and S6 wait for their measurements. Each item is proved as Profiling2 §2 says:
locally on its rows and tests, then committed; the Sweep, the Mantle runs and the full suites are for an acceptance.

## 8. Not to do

- **A JIT, or another engine for its JIT** (§3). Look again only if `run_at` passes 40% of a workload that matters
  after §6.
- **A new expression language.** S3c's simple expressions are a subset of JavaScript evaluated two ways, not a second
  language.
- **Hot Mantle services rewritten in Rust** before M1 says where their time goes. The audit's "native coarse
  helpers" stay an option after S3.
- **A generational collector** now: collection is 3 to 5% of a render.
- **What Profiling2 §6 dropped,** among it leaf URL getters (R53, flat) and chunked arenas (R44).

## 9. Decisions and open questions

**Decided** (the user, 2026-11-10):

- **D1. Expanding with and without localization (W14).** `ec.resource.expand` localizes first by default, as Java's
  does (a text under 256 characters; `ResourceFacadeImpl.expand`), and there is a way not to: `expandNoL10n`, as
  Java has. Both are in the bindings (`script/oxide/resource.rs:56`). What is wrong is which one a `<set value>`
  uses. Java's is a GString, with no localization (Moqui Java's `framework/template/XmlActions.groovy.ftl:83`); here
  the macro emits `ec.resource.expand`, which localizes (`script/runtime/localize.rs:22`), while the native plan
  takes a literal as it is (`script/actions_set.rs:195`). So a body of only sets and the same sets in a larger body
  differ today where a value matches a localized message. The macro is to emit the literal, or `expandNoL10n` for a
  value with `${`. Check the other macros that call `expand` against Java's template the same way.
- **D2. The hasher (W10).** foldhash, everywhere. No crate of ours names a SipHash crate (`siphasher` is in the lock
  file through `phf_shared` and `stun-proto` only); our SipHash is std's default hasher, which about 1,260 lines of
  map and set types still take against 17 that name foldhash. The hot paths of W10 go first, the rest as they are
  touched.
- **D3. The transaction cache as the default (S1): to test.** Turn it on for services with no setting of their own
  and see how things go; if the flush and the switch to read-through hold up, it may be fine as the rule. Some small
  services will only pay for it, so the global setting needs a rule for when a service with no explicit setting
  holds, taken from profiling that shows where it helps and where it only adds its cost. S1 has the test.
- **D4. The fork.** Anything of high value to mr-moqui is fair for moqui/quickjs-oxide. Upstream taking a change is a
  plus and does not decide it: this use is a particular one, and it may be what upstream wants an engine to serve.
  Profiling2 §2's rule stays: no upstream issue or pull request without asking.

- **D5. Row templates need not be byte-equal to today's output (S4b).** The markup may change for other reasons
  after later reviews anyway. Whitespace is content in some documents and must be kept there; in others it is not,
  and stripping it for html and the like could be an option that also simplifies the output.
- **D3, result (Round 60):** on by default for XML-actions and script services; Rust, native and entity-auto services
  and force-new calls keep their definition's attribute.
- **D6. The pool sizes are not fixed (§3, S6).** Round 60: 16 request and 22 service workers, pools of 90. The defaults (eight request workers, 25 service workers) get tuned
  when load testing with real-ish loads begins ([LoadRunner](archive/LoadRunner.md)). If a test before that shows a default
  should change, M4's thread rows for one, change it then and keep tuning.

Nothing is open as of 2026-11-10. A question that comes up in the work goes here.

## 10. Sources

- **[Profiling2](Profiling2.md)** and the archived rounds it lists: every "R*n*", "TX" and "Oxide" citation here.
- **quickjs-oxide-bench** (`../quickjs-oxide-bench`):
  - `rounds/r48-accept/mantle/census-ox1.jsonl`: §2's transaction table, the medians of each operation's `frame`
    lines (`round_trips`, `db_reads`, `micros`), computed for this file; `rounds/r48-accept/mantle-table.md` has the
    two-run times.
  - `rounds/r52/final/table.md`: the `js_speed` rows; `rounds/r52/m1/README.md`: `echo#Data`'s instructions.
  - `work/r53/{base,m3}.log`: the screens' times and instructions; `profile-base.jsonl` and `profile-h4.jsonl`
    through `rounds/r53/profile_table.py`: the phases and host calls; `stacks-B-html.txt`: the `perf` shares;
    `samples3.folded`: the sampler.
- **The action bodies:** every `actions`, `always-actions`, `pre-actions`, `row-actions` and `condition` element in
  the XML under `runtime/components/base` and `runtime/components/addon`, counted and sized for this file: 9,125 elements,
  8,743 distinct, 4,516 of them services' `actions`.
- **The text-keyed lookup:** a std `HashMap<String, _>` of one entry, looked up 200,000 times by a key of 1, 5 and
  16 KB, built with `rustc -O` for this file; not kept.
- **The fork** (`../quickjs-oxide` at b7e34e04): `docs/embedding.md` (the host call's steps and costs),
  `docs/heap.md` (the collector's budget, strings), `docs/performance/execution.md` (inline caches, the loop),
  `docs/planned.md`.
- **[MantleUSL-Performance-Audit](MantleUSL-Performance-Audit.md)** and
  **[MantleUSL-Execution-Benchmark-Plan](MantleUSL-Execution-Benchmark-Plan.md):** the loop counts, the execution
  methods compared (S3c is its method I with a choice per body), Stage B.
- **[LoadRunner](archive/LoadRunner.md):** the order to cash flow for M4 and M8.
