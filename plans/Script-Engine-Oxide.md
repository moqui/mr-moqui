# Script engine: quickjs-oxide beside rquickjs

Status 2026-11-07: Phases 0 to 3 built. quickjs-oxide is a dependency at the fork's revision 87402031, with F1, F2,
F3a, F3 and F4 (§6). Under `script_engine=oxide` every kind runs on quickjs-oxide, and both suites and the client
suite pass; the speed and heap rows miss and are accepted for now (§7, Phase 3 result). The default is still
`rquickjs`, and rquickjs stays until Phase 4.
This plan adds [quickjs-oxide](https://github.com/pocket-nexus/quickjs-oxide), a QuickJS-compatible engine in safe Rust,
first for expressions and other small scripts, then for everything, and then removes rquickjs. It also changes our fork
of quickjs-oxide so compiled code is shared by all threads of the process, as compiled Groovy was in Java. The
measurements it rests on are in §1; the setting that picks the engine is §2; the fork's four changes are §6; the phases
and their gates are §7; how to pick the work up in a new session is §11.

## 1. Why, and what was measured

Round 27 and Round 28 of [Profiling](Profiling.md) left the script engine as the largest cost of a held service call
beyond its round trips. That cost comes in three parts:

- **Compiled code is per engine.** A function compiled in one QuickJS context runs only there, so each thread holds its
  own copy of every module it ran. A copy is about 1.3 times the bytecode, and Forum.js costs 723 KB in each engine.
  The process-wide bytecode cache (`compiled::SHARED_BYTECODE`) saves the parse; reading the bytecode back
  (`JS_ReadObject`) is about 12 times cheaper than compiling (0.44 ms against 5.2 ms for Forum.js), but the copy stays.
- **The bindings work around rquickjs.** The global-names exotic class, the class-id checks, the counting allocator and
  re-entry through `Ctx::from_raw` all use raw `qjs` calls under a pinned rquickjs version.
- **C in the build.** QuickJS is 74k lines of C: `cc` in every build, `bindgen` and the emsdk sysroot for the browser
  build, and cross C toolchains for the mobile apps.

A standalone benchmark (2026-11-07, this Linux machine, release builds with fat LTO, the same JavaScript text in each
engine) compared rquickjs 0.14.0, Boa 0.22.0 and quickjs-oxide 0.4.1 (`3a9ceb96`). It is not kept in the tree; Phase 1
adds the engines to `js_speed` instead. Times are the median µs a call:

| Row | QuickJS | Boa | quickjs-oxide |
|---|---:|---:|---:|
| Engine start | 87 µs, 164 KB | 190 µs, 263 KB | 201 µs, 400 KB |
| Expression from new text each call | 2.1 | 3.8 | 5.1 |
| Condition from new text each call | 3.4 | 8.7 | 8.1 |
| Compiled expression, called | 0.056 | 0.089 | 0.046 |
| 1000 adds to a local | 36 | 25 | 8.0 |
| 1000 adds to a global | 37 | 273 | 10.9 |
| 1000 host function calls | 80 | 85 | 43 |
| 1000 Proxy reads, native trap | 91 | 162 | 82 |
| 1000 string concatenations | 135 | 181 | 166 |
| JSON round trip, 100 objects | 157 | 816 | 170 |
| 100 objects, filter, map and reduce | 44 | 162 | 50 |
| HTML escape of 100 strings (regular expression) | 313 | 1,727 | 452 |
| Template, 20 rows of 7 fields | 81 | 374 | 125 |
| Compile Forum.js (196 KB) | 5.2 ms | 20.5 ms | 8.9 ms |

- **Boa is out.** It is 1.5 to 7 times slower outside a local loop (5.5 times on its own V8 v7 chart, 9 times on its
  RegExp test). It retains 2 to 3 times QuickJS's memory per compiled script. It cannot share compiled code at all: its
  heap is thread-local (`boa_gc`), `Gc<CodeBlock>` is not `Send`, and it has no bytecode format.
- **quickjs-oxide is a good candidate.**
  - It is faster than QuickJS where the interpreter and host calls dominate.
  - It is 1.1 to 1.5 times slower on strings, regular expressions and templates (the screen macros' kind of work).
  - It is 1.7 times slower to compile, and 2.5 times slower on a one-off short expression.
  - It compiled all 369 server-side JS files of the framework, mr-mantle and mr-boss (11 MB) with no failure, in 582 ms.
  - A Proxy as the global object's prototype receives the bare-name `has`, `get` and `set` in the specification's
    order, and its inline caches do not keep a value the proxy changed. That is the mechanism the context stack's bare
    names use.
- **Its design suits sharing.**
  - Every reference in its heap is a 32-bit index, not a pointer.
  - Compiled units are plain Rust data in a `CodeStore` beside the heap, with their constants in small per-runtime heap
    records.
  - Its maintainer's own plan ([#122](https://github.com/pocket-nexus/quickjs-oxide/issues/122)) adds code images: units
    with constants by value, loadable into any runtime.

  After compiling Forum.js, its memory report shows 186 KB of instructions, 135 KB of position tables and 435 KB of
  source text in the code store, all candidates for sharing, against about 190 KB of heap and 57 KB of inline caches
  that stay per runtime.

What it does not do: `with` is a `SyntaxError` and direct `eval` runs as indirect eval (decisions of its maintainer,
`docs/deviations.md`); a runtime has one realm; host functions cannot compile static code or run modules; there are no
embedder exotic objects; there is no C API. The fork changes the parts Moqui needs (§6), and our side stops using `with`
(§5.4).

## 2. The setting

`script_engine` (a property or environment variable, read once, as `screen_tx_cache` is):

| Value | What runs where |
|---|---|
| `rquickjs` | Everything on QuickJS, as today. |
| `oxide` | Everything on quickjs-oxide. Until Phase 3 is done, a script kind the oxide bindings do not cover fails with an error naming the kind, so a test run shows what is left. |
| `small-oxide` | The small scripts of §3 on quickjs-oxide, everything else on QuickJS. |

- **The default.** The target default is `small-oxide`. The default moves from `rquickjs` to `small-oxide` when Phase 2's
  gate passes (§7), so the tree stays green while the oxide bindings are partial.
- **After Phase 4.** rquickjs is removed and the setting is gone.
- **Reports.** `engine_stats` and `engine_memory` report each engine separately: engines started, scripts by kind, heap
  bytes, compiled units. So does the `MOQUI_SCRIPT_STATS_FILE` a test run writes.
- **One engine of each kind per thread, as now.** In `small-oxide` a thread that runs both kinds holds one runtime of
  each, and each is started on first use.

## 3. What is small

A script runs on quickjs-oxide in `small-oxide` when both of these hold:

- **Its entry point is a small kind:**

  | Entry | Small | Notes |
  |---|---|---|
  | `eval_condition` | yes | `condition` attributes, `<if>` conditions, screen and transition conditions |
  | `eval_value`, `eval_field_value` | yes | `from`, `value`, field values in form rows |
  | `expand`, `expand_localized` | yes | `${}` in labels and text |
  | `eval_map` | yes | map literals |
  | Generated XML actions (`run_actions`) | Phase 2 | only when every action is in the subset (`set`, `if`, `iterate` over the bound values) |
  | `run_script` of inline script text, script files (`run_script_location`) | no | until Phase 3 |
  | Templates, screen units (`ScreenHtml.js`, `ScreenQvt.js`, ...), `ec.resource.script` modules | no | until Phase 3 |
  | The test harness's spec engine, moqui-llm's scripts, the devkit's test kit | no | until Phase 3 |

- **Its text passes a static check:** it uses only what the oxide bindings provide. The check is made once per text
  with `oxc_parser` (already used by `script/parse.rs`) and kept with the compiled form.
  - It allows bare names, the operators, literals, member access and calls on values, and the `ec` paths the oxide
    bindings implement. Phase 1 lists that set from a census (§5.3).
  - It does not allow assignment to a bare name in an expression, function or class literals whose value would leave
    the engine, or any `ec` path outside the list.
  - A text that fails the check runs on QuickJS.
  - The route is never changed after a script starts, so a script's side effects happen once, in one engine.

## 4. Two engines on one thread

- **The context stack is shared.** Both engines read and write the host's `ContextStack`, as host values. A JavaScript
  object or function never crosses from one engine to the other: a value that leaves an engine is converted to a host
  value first, as it is today at the end of a script.
- **Tracked values are flushed at the boundary.** QuickJS keeps converted lists, maps and decimals tracked during a script
  and writes them back at a flush. When an oxide script starts inside a running QuickJS script (an expression a host
  call evaluates), the QuickJS engine's tracked values are flushed first: `Flush::Keep` for a read-only small script,
  `Flush::Release` for one that may write. The oxide engine does the same in the other direction once it tracks values.
  `flush_current` already does this for host calls that read the stack; the new boundary uses it.
- **Deadlines.** `script_time_remaining` and the `DeadlineGuard` reach quickjs-oxide through `Host::interrupt`, which it
  polls about every thousand back-edges and calls. An interrupt there is uncatchable, as the QuickJS interrupt handler's
  is.
- **Errors.** Both engines produce the same `Error` with the script's location, line and column, and the same
  `RuntimeError` mapping. The specs that check error text run under both engines.
- **Panics.** A panic in a quickjs-oxide host function poisons its runtime: every later call returns `Poisoned`. The
  engine is dropped and a new one is started on the thread's next script, and the poisoning is logged with the script
  that caused it.
- **Memory.** The oxide runtime adds about 400 KB and its 1 MiB collection floor to each thread that runs a small
  script. With 104 script threads (Round 28's count) that is up to about 140 MiB before F1 and F4. Phase 2's gate
  measures it against Round 28's 139 MiB peak of all engines; a runtime's minimum heap is a `RuntimeOptions` field, and
  the heap cap replaces the counting allocator's cache-pressure check.

## 5. Our side: the oxide bindings

### 5.1 Where the code goes

`crates/moqui-core/src/script/` keeps its public API (`run_script`, `eval_condition`, `expand`, `eval_value`, ...).

- **The seam.** `script/engine.rs` (new) reads the setting, routes each entry (§3) and holds the static check. The
  router is the only code that knows there are two engines.
- **The QuickJS bindings stay where they are** (`script/runtime.rs`, `script/runtime/`).
- **The oxide bindings go in `script/oxide/`.** Its files mirror `script/runtime/` as their parts are written:
  `convert.rs`, `values.rs`, `decimal.rs`, `entities.rs`, `facades.rs`, `compiled.rs`.
- **Shared Rust.** What both engines need and that touches neither engine's types (the context stack, the compiled-text
  hash, the module inertness check, the static check) moves to `script/` beside them.
- **Callers outside the crate.** `with_ctx`, `host_guard`, `facade_of` and `with_ec` hand an rquickjs `Ctx` to moqui-llm
  and the devkit. In Phase 3 they gain oxide forms, and in Phase 4 the rquickjs forms go.

### 5.2 The first subset (Phase 1)

- **Bare names.** The global object's prototype resolves names to the context stack, as `HostGlobalNames` does. It is a
  Proxy with host traps until F2 lands, then a host exotic object.
- **Conversions** between host values and quickjs-oxide values: null, booleans, numbers (`I64` kept as an integer where
  it fits), text, timestamps and dates (as today, millisecond text where the QuickJS bindings give it), lists and maps
  converted on read, decimals as a host object with the `HostDecimal` methods, entity values as a host object with
  their field reads (exotic once F2 lands).
- **The `ec` paths** of the census (§5.3), each a host function or a host object's method.
- **Compiled expressions** are cached per runtime by the text hash `compiled::ScriptText` already computes, in
  `compiled.rs` of `script/oxide/`, until F1 makes the cache process-wide.

### 5.3 The census

Before the `ec` subset is fixed, a full `./moq test-apps` with `MOQUI_SCRIPT_STATS_FILE` records each distinct small
script: its entry kind, its text hash, and the `ec` paths and host functions it reached. That is one row per text, with
a count of runs. The census gives the paths that cover nearly all runs of small scripts. The static check's list is
those paths, and the rest of the small scripts run on QuickJS until Phase 3 binds them.

### 5.4 Templates without `with`

The snapshot form of a template (`template.rs`, `Scope::Snapshot`) wraps its body in `with ($scope($data))`; the live
form does not use `with`. The snapshot form changes to the live form's shape: its names resolve through a global-names
object over the snapshot's data, in a context frame of its own. It is checked on QuickJS first, in its own commit, so the
template tests show any difference before quickjs-oxide runs templates.

## 6. The fork: four changes

The fork is [moqui/quickjs-oxide](https://github.com/moqui/quickjs-oxide), cloned beside this repository at
`../quickjs-oxide`, from upstream's `main` at `3a9ceb96` (the revision measured in §1). It tracks upstream
(`pocket-nexus/quickjs-oxide`, the clone's `upstream` remote). Each change is a branch from upstream's `main`, so
it can go to upstream as a pull request, and is merged into the fork's `main` for our use. The workspace depends on
the fork by git revision, pinned exactly as rquickjs is now:

```toml
quickjs-oxide = { git = "https://github.com/moqui/quickjs-oxide", rev = "<revision>" }
```

Local work on the fork and the bindings together uses a `[patch]` in an uncommitted `.cargo/config.toml` or the
workspace manifest, pointing at `../quickjs-oxide`; a commit of this repository never carries the path. Each change below is proposed upstream as an issue first, then as
pull requests that pass upstream's gates (their `AGENTS.md`: the Sweep, and the area's extra benchmarks). Whether
upstream takes a change is up to them; where they choose a different design, the fork moves to theirs.

### F1. Compiled code shared by the process

The goal is that a script compiled once on any thread runs on every thread with no parse, no image decode and no copy
of its instructions, source or positions. A runtime keeps only what is its own: constants as heap values, inline caches,
closures and atoms.

What stands in the way today:

- A `CodeUnit` holds its runtime's cache base (`ic_base`).
- Instructions name property keys by the runtime's atom index (`Insn::imm` read as a `PropertyKey`), and `name_atom` is
  a runtime string.
- `children` are ids in the runtime's own `CodeStore`.

In steps, each measured:

1. **Code images** (upstream's #122 §4.1 and §4.2). Units with their constants by value (atoms by text, numbers, BigInt
   digits, regular expressions as source and flags), loaded into a runtime by interning the constants and publishing the
   units. This is upstream's own plan. On our side it replaces `SHARED_BYTECODE`: the image is kept per text for the
   process, and a new runtime loads it instead of compiling.
2. **Shared unit bodies.** The parts of a unit that name nothing of a runtime (handlers, captures, flags, counts,
   position tables, source text) are held once in an `Arc`'d, append-only process store. A runtime's code store entry
   holds a reference to the shared body, its own instructions with the keys patched for its atoms, its cache base and
   its name atom. Loading is a copy of the instructions and a key patch, with no decode. Upstream's memory plan already
   takes the source text from 2.2 times the file to once (item B).
3. **Shared instructions.** Property keys in instructions become independent of the runtime, so instructions are shared
   too. There are two candidates, chosen by upstream's instruction gate:
   - a unit-local key index that each runtime maps through a per-unit key table built when the unit is instantiated;
   - process-wide atom ids, where every runtime honours one append-only atom table for code keys.

   The cache base moves to a per-runtime table indexed by unit id, read at frame entry (two sites in `vm/interp.rs`).

Bounds: the process store keeps code for the process's life, under a byte cap like today's 64 MiB. A text that runs
once, or one past the cap, stays transient in its runtime (upstream's `EvalOptions::transient`). The process store is
keyed by text hash, text, file name and strictness, as `compiled.rs` keys today.

Acceptance:

- After step 2, a second runtime's cost for Forum.js is at most 300 KB, against about 1 MB on today's oxide and 723 KB on
  QuickJS.
- After step 3, it is at most 120 KB.
- A thread's first run of a script another thread compiled does no parse.
- Upstream's instruction gate holds.
- Over the full mr-mantle run, the JS heap of all engines at its peak is under half of Round 28's 139 MiB.

### F2. Exotic objects for the embedder

Host objects whose get, set (with its receiver), has, delete, get-own-property, define-own-property and own-keys are
Rust hooks, as QuickJS's `JSClassExoticMethods` are and as `#[rquickjs::class(exotic)]` gives us today.

- **The API.** A host class registers its hooks once; `new_host_exotic(value, class, prototype)` makes an object of it.
- **In the engine.** The object is an exotic kind: the property walks take the internal-method path for it, as they do
  for a Proxy, and the inline caches never fill for it.
- **Uses in Moqui:**
  - the global object's prototype for bare names (`HostGlobalNames`);
  - entity values (`exotic_module!` in `script/runtime/entities.rs`), where a set's receiver matters (`keep_assigned`);
  - the context maps that convert lazily.

Upstream fit: it is part of the C API's surface that upstream lists as not yet supported, and does not change
semantics.

Acceptance: 1000 bare-name reads and writes through the prototype cost no more than they do on QuickJS's exotic class
today (Round 11's 464 µs for 1000 adds to a bare name, on the M1 Pro); Test262 and upstream's Proxy corpus are
unchanged.

### F3a. Re-entry from deep Rust

On upstream, a `Context` or `Runtime` call made while a host function runs returns `Reentered`; only the host
function's own `Scope` re-enters. Moqui's nested runs (a script calls a service that runs a script) and the spec
engine's promise jobs are reached from Rust code that holds no `Scope`. `Scope::lend(f)` (and `ExoticScope::lend`)
hands the engine to `f`: while it runs, `Context` operations and the runtime's job operations work above the host
function's frames, and lends nest. A lend costs about 0.2 µs, so the bindings lend only around host methods that may
nest (service calls, entity operations), not around every host call. Until a binding lends, the router sends a nested
small script to QuickJS.

### F3. Compiling and running from a host function

Today, inside a host function, `compile` and `eval` make transient code (5% more instructions across V8 v7, 15% on
DeltaBlue), and `compile_module` and `execute_module` return `Unsupported`. Moqui compiles while a script runs:

- a service a script calls runs a script in the same runtime (Round 28's nested runs);
- `ec.resource.script` loads a module;
- a screen render compiles its units.

With F1's append-only process store, publishing from inside a running frame is safe: units are added and never moved, so
the loop's borrow of the running unit is not disturbed. So `Scope::compile` publishes to the shared store, as the
top level does. Running a module from a host function follows, if any Moqui caller needs it (none does today:
`ec.resource.script` runs a script, not an ES module).

The native-stack budget (about 725 levels of Rust to JavaScript to Rust in the default 1 MiB) is set from the thread's
stack size; Moqui's deepest nesting in the mr-mantle run was 18.

Acceptance: a nested script and a module load from a host function run at the static code's instruction count; the
nested-run tests of Round 28 (`OuterRun`, the screen mode's `enter` and `leave`) pass on quickjs-oxide.

### F4. Speed and footprint where Moqui's scripts spend their time

These are measured on `js_speed` rows and taken upstream as ordinary pull requests under their gates:

| Area | Moqui's row | Today, quickjs-oxide ÷ QuickJS | Target |
|---|---|---:|---:|
| `String.prototype.replace` with a regular expression and a callback, the regexp engine | HTML escape | 1.45 | 1.1 |
| Concatenation and template literals | string concatenation; template rows | 1.23; 1.54 | 1.1 |
| `JSON.stringify` and `JSON.parse` | JSON round trip | 1.08 | 1.0 |
| Compiling a short script (115 allocations for `a + b * 2`) | expression from new text | 2.4 | 1.2 |
| Runtime start and floor (2,985 allocations, 400 KB) | engine start | 2.3, 2.4 | 1.5 |

The screen rows (`List html`, `List qvt`, 100 rows) are the end-to-end check: about half their time is the screen
macros' JavaScript, much of it strings and regular expressions.

## 7. Phases

Each phase ends with its gate; a phase does not start until the one before it passes, except that the fork's changes
proceed in parallel and land in the order the phases need them.

| Phase | Work | Gate |
|---|---|---|
| 0. Fork and build | The fork (`moqui/quickjs-oxide`, §6) as the workspace dependency (git revision, `[patch]` to `../quickjs-oxide` for local work); the workspace's Rust to 1.99, which quickjs-oxide needs. Build checks for the host, the browser build (`wasm32-unknown-emscripten`) and the mobile targets. The license review entry (MIT, and its NOTICE). | `./moq build`, the browser build and `cargo lint-min` pass with the dependency in; nothing calls it yet. |
| 1. The seam and conditions and values | `script/engine.rs`, the setting, the static check; `script/oxide/` with bare names (a Proxy), conversions and the census's `ec` paths; `eval_condition`, `eval_value`, `eval_field_value` routed in `small-oxide`; the census; `js_speed` with `JS_SPEED_ENGINE` and its expression rows on both engines. | `./moq test-apps` and `./moq test-framework` pass under `script_engine=small-oxide`. The `js_speed` expression rows are no slower than on QuickJS. |
| 2. More small kinds, F1 steps 1 and 2, F2 | `expand`, `eval_map` and the subset of generated actions; the exotic global names and entity values (F2); the process-wide image and shared bodies (F1). The default moves to `small-oxide`. | Both suites pass under `small-oxide` and `rquickjs`. `service_speed`'s `echo#Data` and `js_speed`'s actions rows are no slower. The full mr-mantle run's JS heap at its peak is no more than Round 28's. |
| 3. Everything on quickjs-oxide | Bindings for script files, services, templates (§5.4 first), screen units, `ec.resource.script` modules, the spec engine, moqui-llm and the devkit's test kit; F1 step 3, F3 and F4. | Both suites and the client suite pass under `script_engine=oxide`. `js_speed`'s screen rows are within 10% of QuickJS. The Mantle operations against the Postgres 200 µs away (Round 26's table) are no slower. The full mr-mantle run's JS heap at its peak is under half of Round 28's. |
| 4. rquickjs removed | `oxide` becomes the only engine; `script/runtime/`'s QuickJS bindings, the setting and the rquickjs dependency go; the browser build loses `bindgen` and the emsdk sysroot's QuickJS needs; docs and `Parity-Diffs` updated. | `./moq test-all`, the browser build and the packages pass with no C in the script engine's build. |

### Phase 1 result (2026-11-07)

- **Built:** the seam (`script/engine.rs`, `check.rs`, `census.rs`, `run.rs`, `text.rs`), the engine-neutral host
  table (`script/host/`) with a binder for each engine, and the oxide core (`script/oxide/`). `ec.message`,
  `ec.logger`, `ec.cache`, `ec.user`, `ec.l10n` are on the table.
- **Census** (release, mr-mantle specs): 86% of condition and value runs went to oxide, with no fallbacks. The largest
  QuickJS shares were `toNumber`, `sort` (refused as a mutator) and `ec.user.userAccount`.
- **Speed gate missed.** `js_speed` expression rows, µs a call, QuickJS → oxide: `a` 0.40 → 0.68, `a + b * 2`
  0.55 → 1.02, `item.name` 1.18 → 1.66, a new map 1.93 → 3.29, condition 0.60 → 1.21, new text each call 0.64 → 1.19.
  The cost is the global Proxy's traps on every bare name and a fresh copy of lists and maps on each read; Phase 2's
  exotic global names (F2) and tracked values address both.
- **What the static check refuses**, beyond §3: mutating methods whatever the receiver (`push`, `sort`, `splice`,
  `assign`, ...), since oxide does not yet write converted values back; `constructor`, `__proto__`, `prototype`, the
  legacy accessor definers and `then`; `Reflect`, `Proxy`, `Promise`, `Symbol`, `Function`, `eval`, `globalThis`;
  arrow functions except as the argument of a fixed list of array methods. A text that falls back twice on a thread
  goes to QuickJS directly. The oxide runtime's lockdown for these (throwing function constructors) is for small
  scripts only; `oxide` mode needs a runtime without it.
- **Differences found:** oxide's error wording (`'x' is not defined`) and columns differ from QuickJS's; its
  default host is UTC, so `MoquiHost` gives local time.

### Phase 2 result (2026-11-07)

- **Built:** `expand`, `eval_map` and the actions subset on oxide in `small-oxide`; tracked values written back
  (`Keep`, `Release`, `End`) and the cross-engine flush both ways; the global names and entity values as F2 exotic
  objects; F1's code images and shared unit bodies; the difference check (Expression profile only). Every facade is on
  the host table (`script/host/`), so both engines bind the same methods.
- **Suites:** `./moq test-apps` (3306 features, 17 client walks) and `./moq test-framework` pass under `small-oxide`
  and `rquickjs`, apart from two Rust tests that fail on `master` before this work:
  `serve::a_server_that_does_not_start_fails_its_suite_only` and
  `the_wrapped_ec_takes_probes_and_leaves_the_framework_the_real_one`.
- **Census** (`small-oxide`, `./moq test-apps`): oxide ran 100% of `eval_condition`, 99.9% of `expand`, 99.8% of
  `eval_map`, 99.6% of field values, 87.2% of `eval_value` and 41% of `run_actions`; no fallbacks, no differences.
- **Heap gate passed.** Peak JS heap of all engines in the `./moq test-apps` run: 138.4 MB under `small-oxide` (44.0
  MB of it oxide), 139.6 MB under `rquickjs`; Round 28's peak was 139 MiB (145.8 MB).
- **Speed gate missed.** `service_speed` `echo#Data`, µs a call: 11.5 to 11.8 on QuickJS, 13.7 to 14.1 under
  `small-oxide`. `js_speed` expression rows, QuickJS → oxide: `a` 0.39 → 0.43, `a + b * 2` 0.56 → 0.65, `item.name`
  1.23 → 1.37, a new map 1.98 → 3.12, decimals 1.31 → 1.63, `itemList.length` 57 → 77, condition 0.59 → 0.85, `${}`
  1.34 → 1.81, new text each call 0.55 → 1.23. The actions rows run on QuickJS in `js_speed` either way. The rest is
  the bindings' per-call cost (allocations: 0 → 4 for `a`, 11 → 53 for a new map) and compile time for new text, F4's
  work and Phase 3's.

### Phase 3 result (2026-11-07)

- **Built:** script files, script services and `run_script` on a whole oxide runtime (nested runs through F3a, tracked
  values, errors with their location, the deadline); `ec.resource.script` modules; templates (§5.4's snapshot form
  first, then the locked sandbox); XML actions and screen macros generated on oxide; screen units, `render_inline_text`
  and the `screen_runner`; the spec engine, `settle` through F3a; moqui-llm's `script_object`; the widget check's
  wrapped `ec`; QuickJS's own builtins (`queueMicrotask`, `atob`, `btoa`, `performance`, `DOMException`,
  `SuppressedError` and the disposable stacks) on a whole runtime; the helper objects frozen on both engines; resource
  objects let go when the outermost script ends (§ Engine differences); memory pressure, out-of-memory recovery and
  restarts (§ Memory). The fork's F4 speed work is merged (V8 v7 instructions ×0.974 of the F1 to F3 revision, RegExp
  ×0.805), with RegExp searches polled for the deadline by their work.
- **Suites passed** under `script_engine=oxide`, and again under `rquickjs` on the same commit:
  - `./moq test-apps`: 3306 of 3306 features and 17 of 17 client walks.
  - `./moq test-framework`: every spec suite passes. The cargo step fails only on
    `serve::a_server_that_does_not_start_fails_its_suite_only`, which fails on both engines and on `master` before
    this work.
  - The 26 unit tests of QuickJS-only entry points are pinned to `rquickjs` (§8).
  - `./moq accept-all`: under `oxide` it fails nothing that does not fail under `rquickjs` on the same commit.
    - Three oxide-only failures were fixed: SimpleScreensRender, the Tools AutoFind screens, and TuiRender's out of
      memory.
    - The first two had one cause: a form-list row read the first row's value kept by name.
    - The failures left on both engines are application specs, worked on separately.
- **Speed rows missed, accepted.** The cost is in the bindings between the host and the engine (host getters, context
  reads, allocations per call); plain JavaScript is as fast or faster on oxide. The fork's API can grow to fit them.
  - `js_speed` screen rows are ×1.03 to ×1.60 of QuickJS against the gate's ×1.10:
    - Plain html ×1.03, List text 100 ×1.09, List html 100 ×1.17, List.qvt 100 ×1.26.
    - Labels ×1.37, Single html ×1.50, Single.qvt ×1.60.
  - Script rows: a local loop ×0.22, the empty script ×0.80, walking 100 context maps ×3.3, a host getter ×2.2.
  - The Mantle operations against the Postgres 200 µs away (Round 26's nine services, the transaction cache on): ×1.039
    in all, 3458 ms on QuickJS and 3594 ms on oxide. The services range from ×1.006 to ×1.11 (`apply#Settlement`).
    Round trips are the same on both engines.
- **Heap row missed.** The peak of each `./moq test-apps` worker's script heaps:
  - `oxide`: 91 to 118 MB.
  - `rquickjs`: 126 to 140 MB.
  - The gate is half of Round 28's 139 MiB. What is left is the fork's: about 30 runtimes a process that share no
    caches, and arenas that keep their interior free space (§ Memory).
- **Left for later:**
  - The fork's `fix/lower-heap-oom` (3f5d5809). It reports a compile that runs out of heap while lowering as out of
    memory. Our side maps it already (`errors::compile_out_of_memory`), so the revision is not bumped for it.
  - A decimal read again after a releasing flush is a new object on oxide (`===` is false), where QuickJS gives back
    the same one.
  - `entity_proxy`'s scan grows with a run's rows.
  - `using` declarations, which need the fork's parser.
  - A back-reference compared in one unpolled step.
  - On both engines, a filter's `try`/`catch` around `chain.doFilter()` cannot keep a servlet's error from the
    response.

### Phase 3: engine differences

What `oxide` still shows differently from QuickJS, found by the generation comparison
(`screen::generate::tests`) and kept on purpose or left to the fork:

- **Stack columns.** quickjs-oxide places a call's column at its `(`, QuickJS at the callee's name
  (`eval_script:3:48` against `3:43`); the lines agree. The generation tests compare stacks without columns. Fork.
- **Native messages.** `stack overflow` where QuickJS says `Maximum call stack size exceeded`, `cannot convert to
  object` where it says `Cannot convert undefined or null to object`. Fork; not worked around.
- **Out of memory.** oxide's `InternalError: out of memory` is one preallocated error with no stack, where QuickJS's
  has the frame that ran out (`Error: out of memory\n    at hungry (eval_script:4:87)`).
- **A thrown value that is not an error** (`throw "x"`, `throw {a: 1}`): oxide prints the value (`"x"`, `{ a: 1 }`),
  QuickJS rquickjs's Debug form (`String(Ok("x"))`, `Object(0x…)`, an address). Kept: oxide's is the readable one.
- **Type names in conversion errors** (`Error converting from js '<type>' into type 'string'`): oxide names them as
  rquickjs does, from the engine (`errors::rquickjs_type_name`), with two gaps. A whole number held as a float
  (`1.5 * 2`) may be `float` on one engine and `int` on the other. A Proxy is `proxy` only where the walker's runtime
  noted it; an element literal's attribute that is a Proxy is `object` on oxide.
- **RegExp interrupt, until the fork rev with f4-regexp-interrupt** (integ/moqui-3): before it, the fork's RegExp
  exec polls no interrupt, so a backtracking pattern runs past the script deadline and
  `script::theme::tests::each_budget` hangs under `script_engine=oxide`.
- **No `with`, and `eval` is always indirect** (the fork's deviation ledger): a direct `eval` does not see the
  calling function's names. The test harness's `runInContext` (testkit/server.js) keeps its `with` form on QuickJS;
  on oxide it runs the source as the body of a function whose parameters are the sandbox's own names
  (`testkit/sandbox.rs` keeps eval's completion value) and writes back the names whose value the source changed. A
  known limit: a function the source made sees the names as they were passed, and its later assignments to them
  stay in it, where `with` would reach the sandbox; a write through `globalThis` is the sandbox's at once but a bare
  read of the same name in the source still sees the passed value. A `"use strict"` directive stays first, so it
  applies.
- **`settle`** (`assert.rejects`) attaches its handlers and runs the queue's jobs, oldest first, until they run
  (`EngineOps::run_pending_job`), on both engines. QuickJS's form read the promise's state directly and returned at
  once for a promise already settled; now every job queued before the handlers' runs first, unrelated ones too, and
  a promise already settled costs one more job.
- **`runInContext`'s `with` form on QuickJS** is now compiled by an indirect `eval` at global scope (so that
  quickjs-oxide, which refuses `with`, can load server.js), not written in the prelude's function: it no longer
  closes over the prelude's names, which it did not use.

### Phase 3: the spec engine, moqui-llm and the devkit

- **The spec engine.** Under `oxide` the test harness's spec engine is a quickjs-oxide runtime of the thread's own
  (`script/oxide/spec.rs`), for whole scripts (no small-script lockdown), with `ec`, the shims and the bare names as
  the main runtime has them, and `System` and the helper objects as globals. A spec's host call that may run a
  script lends the runtime, and the script runs on the thread's main runtime as a run of its own: the conversions
  follow the innermost runtime entered (`oxide::engine::Active`). `__moquiTestHost` is a host-table class
  (`testkit/host.rs`), installed the same way on both engines, so the devkit no longer uses rquickjs.
- **moqui-llm.** OpenUI Lang's parser (`ui/openui.rs`) and the Vue SFC script check (`ui/vue_sfc.rs`) run in a
  runtime of their own with no `ec` (`ui/standalone.rs`), quickjs-oxide under `oxide`. `ec.llm` was already on the
  host table.
- **What stays on rquickjs until Phase 4**, whatever the setting:
  - `screen/vue_check.rs`: the vendored Vue compiler (`compiler-dom.cjs.js`) in a QuickJS runtime of its own, for
    Gate 3's check and the server-static compile (and moqui-llm's template check through it).
  - `tests/browser_libs.rs`: the vendored browser libraries run in QuickJS (the Vue compiler bundle).
  - `screen/tests/qvt.rs`: a test that runs the precompiled qvue bundle in a QuickJS runtime of its own.
  - `moqui-host/tests/js_speed.rs`: the benchmark, which measures both engines.
  - The public functions that hand out an rquickjs `Ctx` or value (`with_ctx`, `with_spec_ctx`, `host_guard`,
    `facade_of`, `with_ec`, `host_to_js`, `js_to_host`, `text_upload_to_js`): no crate outside moqui-core calls them
    now; they go with the QuickJS bindings.

### Engine differences under `oxide` (Phase 3, scripts, modules and screen units)

- `ec.context` on oxide is only `{get, formListFindId, orderByField}`, the members the shims and forms read, as
  QuickJS's object is. The helper objects (`ObjectUtilities`, `CollectionUtilities`, `StringUtilities`, `WebUtilities`,
  `UserSecurityChecks`) and `System` are globals of a whole oxide runtime, from `host::globals`. `ResourceReference`
  (`{isTextFilename}`) is the tools shim's global on both engines' whole runtimes, read-only on each
  (`resource_reference_is_a_global_of_each_whole_runtime`); the small scripts' static check still sends a text naming it
  to QuickJS under `small-oxide`. On both engines the helper objects, `System` and the functions on them
  are frozen, so a script cannot replace one (a `UserSecurityChecks` check among them) for the later scripts on the
  thread.
- A bare assignment to a helper's name (`StringUtilities = x`) writes the context stack on QuickJS; on oxide the name is
  a read-only global and the assignment throws a TypeError. A context value named after a helper (a request parameter
  `UserSecurityChecks`, say) hides the helper on QuickJS, whose bare names read the stack first; on oxide the global
  wins. On QuickJS a check so hidden fails closed: the stack's value is not callable (a function does not convert to
  the stack), so the write the check guards fails while the value is on the stack (`service/eca_tests.rs`,
  `the_security_checks_run_on_each_engine`).
- The server-static widget check's wrapper of `ec` (`runtime::WRAPPED_EC`) is on both engines (`oxide/widget.rs`): a
  widget expression (a condition, value, map or `expand` template, its value converted inside the wrapper),
  `ec.resource.expand`, `expression` or `condition` runs with it; a screen unit, a script, actions, a live-form
  template function (`FancyRenderer`, `render_trusted`) and `ec.resource.script` with the real `ec`, as on QuickJS. A
  wrapper that cannot be taken off drops the thread's runtime (either engine) when its outermost run ends, as a
  poisoned one goes; the next script starts a new one. A run nested in a wrapped expression (a host call that runs a
  script) gets the real `ec` for its run on both engines; on QuickJS that had been lost since Round 28 put nested runs
  in the one engine (`the_wrapped_ec_takes_probes_and_leaves_the_framework_the_real_one`). Under `oxide` the expression
  a violation names is set at the entry point (`note_widget_expression`), since oxide sees only the text as it runs. The
  error's stack names `wrapped_ec` on oxide where QuickJS names `eval_script`.
- A Set the stack holds is a live JavaScript `Set` under `oxide`, tracked and written back; `small-oxide` still runs such
  a text on QuickJS.
- What a script holds is let go at a fixed point on both engines, never at a collection. QuickJS frees an object by its
  count when the script lets it go; quickjs-oxide collects by tracing, so an object can live long after its script.
  So:
  - A request owns its spooled uploads (`web/multipart.rs`, `ReleaseAtEnd` in `screen::serve_at`): their files go
    when the request ends, on both engines, whatever still holds a copy (a script object, a context value, an async
    service's parameter), and a copy read later fails (`the upload x is no longer available: its request has ended`).
    Before, a file went with the last copy, so under oxide it stayed until a collection (`no spool file is left`).
  - When the outermost script on the thread ends (`run::enter_depth`'s guard, either engine), quickjs-oxide lets go the
    state of each object of a resource class made since it started (`host::released_at_run_end`: `UploadedFile`,
    `InputStream`, `SftpClient`, `ServiceFuture`; `bind::release_run_objects`), the objects still reachable or not: a
    stream's buffer of a whole file, an upload's view of its spool file, an SFTP client (closed with an error in the
    log, as Java's `finalize`), a future's results. A later call of the object's methods, or passing it as an argument
    that takes the object, throws `this X was let go when the script that made it ended`; a value that holds one (a
    list written back at a script's end) converts it to null. A `ReleaseScope` (`script::run`) holds this release over
    several outermost scripts: the spec runner opens one for each spec (`testkit::run_spec`), whose registration,
    `setupSpec`, features and `cleanupSpec` are runs of their own sharing the file's objects; only the release waits,
    each run's own end (write-back, heap sample, deadline, unread bodies) stays its own. QuickJS has let such an object go by then unless a
    script kept it past its end (on the global object), where it still works; the host's own copies are not touched
    on either engine.
  - The response bodies a script opened with `send()` and did not read go when the outermost script ends on both
    engines; under oxide they had stayed (only QuickJS's run cleared them).
  - Nothing a script can hold keeps a database connection, a transaction or a lock: an entity list (also
    `iterator()`) is its rows, read whole; a find is a builder; a `dbresource://` stream reads its chunks per call;
    transactions are the thread's. An entity list's rows (and other plain values) are memory only and still wait for
    the collector on oxide; the heap's pressure check does not count them.
- A decimal or entity value a script reads by name is one object until the next host call that may change the stack
  (a `Release` flush): QuickJS reads an entity value from the stack each time and releases a decimal there. Oxide had
  kept it for the whole run, so in a render (one run) each form-list row read the first row's row-action value
  (mr-marble PostedBalanceSummary's class names, the Tools AutoFind and AutoEditDetail lists).
- Error columns: oxide places a call at its `(`, QuickJS at the callee's name.
- An assignment through a list on the prototype chain (`Object.create(list).x = 1`) defines on the receiver on both
  engines now; oxide's list ignored it, as it ignores one to the list itself.
- The builtins QuickJS has and quickjs-oxide lacks (`queueMicrotask`, `atob`, `btoa`, `performance`, `DOMException`,
  `SuppressedError`, `DisposableStack`, `AsyncDisposableStack`, and `Symbol.dispose` and `Symbol.asyncDispose`) are
  made in JavaScript on a whole oxide runtime (`oxide/builtins.rs`), as builtins (writable, configurable, not frozen),
  and answer as QuickJS's do (`the_quickjs_builtins_are_on_each_whole_runtime`). What still differs:
  - `using` and `await using` declarations are syntax quickjs-oxide's parser does not have (`SyntaxError: expecting
    ';'`); QuickJS runs them. Fork.
  - `performance.timeOrigin` is the runtime's start in epoch milliseconds on oxide, a monotonic clock reading on
    QuickJS; `performance.now()` counts whole milliseconds on oxide, fractions on QuickJS.
  - The prototypes' own keys come in another order (`constructor` first, as a class makes them), and a
    `SuppressedError` has an own `stack`, as every oxide error does.
  - `Error.captureStackTrace` and `Error.prototype.stack` are QuickJS's only.
  - A `queueMicrotask` callback that throws: QuickJS's job throws and rquickjs's `execute_pending_job` drains the
    exception; oxide's callback runs after an `await`, so the throw rejects a promise nothing holds. Neither reaches
    `settle` or the run, so a spec sees the same.
  - The small-script runtime (`small-oxide`) does not have them; the static check sends a text naming one to QuickJS.

### Memory under `oxide` (Phase 3)

The app suites under `oxide` ran the main thread's runtime into its 64 MiB cap and failed every later script with
`out of memory` (2604 of 3306 features passed). The screen unit cache kept QuickJS's bounds (4096 units, 8 MiB of
source), but once the static code store is full (4 MiB of source, mostly units) a unit's code is on the heap, about 3.5
bytes for each byte of source: 25 MB of code for 7 MB of units. And the heap's arenas give memory back only from their
ends, so the reserved bytes, which the cap counts, stay two to three times the live ones after a run that made many
values. Now the unit cache keeps 1024 units and 2 MiB of source; at half the cap the runtime collects, then lets its
caches go, then starts again on the next script (`restarted` in the stats); and a script that runs out fails alone, its
compile tried again once the caches went. `MOQUI_SCRIPT_HEAP_LOG=<file>` appends a line of the thread's runtimes'
heaps after each spec. The fork could give back interior free space (a compacting sweep, or arenas in chunks).

A screen render is one outermost run around many nested ones, so two gaps let a long sweep (mr-boss TuiRender at
Phase 3's gate) reach the cap: the pressure check read a sample up to 32 runs old, and a unit that ran out inside a
render (its error written in the page as a section's) did not count, so the next render started at the cap too. Now
each outermost run's end samples the heap unless the last sample is under 250 ms old; anything that runs out inside the
outermost run (a unit's compile or call, a nested run) has its end relieve the heap as for a run that ran out
(`OxideEngine::note_out_of_memory`); and a unit's compile that runs out is tried again once the caches went, as a
script's is. quickjs-oxide gave a compile that ran out while it lowered the code as `Internal("out of memory")`, not
`OutOfMemory`, so neither retry fired; the bindings take that text as the cap's (`errors::compile_out_of_memory`), and
the fork's fix is on its branch `fix/lower-heap-oom`.

## 8. Testing

- **Unit tests for the bindings.** The tests in `script/` that check binding behaviour (conversions, tracked values,
  nested runs, errors) run under each engine from one body: a test helper sets the engine for the test's thread. A
  test of a QuickJS-only entry point (`with_ctx`, the QuickJS walker, binder or engine internals) pins its thread to
  `rquickjs` (`engine::quickjs_for_test`, in each thread it spawns too), so `script:: screen::` passes under every
  setting; those tests go with the QuickJS bindings in Phase 4.
- **The difference check.** `script_engine_check=true`, for development and test runs only, runs each small script on
  both engines and logs a difference in result or error. That shows where the oxide bindings give a different value
  before a user sees it. It is off by default and in every package.
- **Full runs per setting.** `script_engine=small-oxide ./moq test-apps` is the normal run from Phase 2. Runs under
  `rquickjs` and `oxide` are made at each phase's gate, and before a commit that changes the seam or the bindings. The
  regression budgets in `test/budget` hold under every setting.
- **Acceptance suites** (`./moq accept`) run at Phase 3's and Phase 4's gates, or when the user asks.
- **Speed.** `js_speed` gains `JS_SPEED_ENGINE` (`rquickjs`, `oxide`) for each section. Its rows are the gates above, and
  the table of §1 is replaced by its numbers in [Profiling](Profiling.md) as each phase lands.

## 9. Risks

- **A young upstream.** quickjs-oxide began in July 2026, is at 0.4.x with one maintainer who decides its deviations, and
  is mostly built by agents. Its API will change. The fork pins a revision and follows upstream at chosen points, and our
  bindings use only its public embedding API outside F1 to F3's additions.
- **Divergence.** If upstream declines F1 or takes another design, the fork carries ours until it can adopt theirs. F1 is
  built on upstream's own planned code images so the first steps line up.
- **Semantic differences between the engines in `small-oxide`.** These include number formatting, date conversion,
  error messages and property order. The difference check (§8) and the static check's narrow subset bound them; a
  difference found is fixed in the bindings or listed in `Parity-Diffs` with its reason.
- **Memory of two runtimes per thread** until F1 and F4 land (§4). It is measured at Phase 2's gate.
- **Compile speed** for script text that changes on every call, which no cache helps. The census shows how much of it
  there is; F4's short-script compile is the fix.
- **Upstream's gates are slow to run.** Their Sweep measures serially with nothing else running. A fork change is
  measured on a quiet machine before it is proposed.

## 10. Decisions

Made:

- quickjs-oxide, not Boa (§1).
- Two engines behind `script_engine`, with `small-oxide` the target default, and rquickjs removed at the end (§2, §7).
- A fork that proposes its changes upstream and adapts to upstream's designs where they differ (§6).
- No `with` in Moqui's generated code (§5.4).

Open, with what decides each:

- F1 step 3's key scheme (unit-local key tables or process-wide atom ids): upstream's instruction gate, and upstream.
- Whether generated actions join the small kinds in Phase 2 or wait for Phase 3: the census, and the `echo#Data` row.
- The process store's byte cap and when a text counts as one-off: the census's counts of distinct texts.

## 11. Resuming

State at the end of Phase 3 (2026-11-07):
- **mr-moqui:** `master` holds Phases 0 to 3, Profiling Rounds 33 to 38 and the pin to the fork's 87402031.
- **The fork:** its `main` is 87402031 (Round 38's `perf/visit-plain-data` merged).
- **Defaults:** `rquickjs` is the default. `small-oxide` and `oxide` are both whole and pass the suites.
- **Measurement:** the measurement workspace is `../oxide-bench` (its `README.md`). It has:
  - the fork's Sweep;
  - the scripts for the Mantle operations against a Postgres 200 µs away;
  - Round 32's raw data.

The work goes on in this order: first the bindings' speed and the heap (Profiling's Round 32 "Next"), then Phase 4.

### Where things are

- **Engine choice.** `script/engine.rs` holds the setting and `route(kind, text)`. `script/run.rs` holds the depth, the
  deadline and `ReleaseScope`.
- **Bindings for both engines.** `script/host/` is the host table both engines bind:
  - `Method`, `HostClass` and `EngineOps`;
  - the helper objects in `host/globals.rs`.

  A new helper method or `ec` method goes on the host table. `runtime/ec_extra.rs` is QuickJS's old binding and goes
  in Phase 4: work added only there (as Admin P3 and P4 did) is missing on both engines, because the host table's
  helpers replace and freeze them.
- **The oxide engine.** `script/oxide/` is the oxide engine:
  - `engine.rs`: the runtimes, `with_run`, memory pressure and restarts;
  - `tracked.rs`: tracked values and `kept`;
  - `bind.rs`: the binder, and the objects let go at a run's end;
  - `units.rs`, `templates.rs`, `walker.rs`, `spec.rs`, `builtins.rs`, `widget.rs` and `errors.rs`.
- **The QuickJS engine.** `script/runtime.rs` and `script/runtime/` are QuickJS's engine and bindings.
- **Tests for both engines.** `engine::on_both_engines`, `with_script_engine` and `quickjs_for_test` are the test
  helpers (§8).
- **Diagnostics:**
  - `MOQUI_SCRIPT_STATS_FILE=<file>` writes a line per process with scripts by kind and engine, fallbacks and heap
    peaks.
  - `MOQUI_SCRIPT_HEAP_LOG=<file>` writes a line per spec with each runtime's arenas, code and caches.
  - `script_engine_check=true` is the difference check.

### Measuring

- **`js_speed`.** One release build, then each engine in turn, two runs of each; the rows are compared at their
  better run. Release builds only, with nothing else building.

  ```
  JS_SPEED_ENGINE=rquickjs|oxide JS_SPEED_ONLY=screen,script,template \
    cargo test --release -p moqui-host --test js_speed -- --ignored --nocapture
  ```

  `JS_SPEED_ROW=<part of a row name>` runs those rows alone, for a profile (`samply`, `perf record -g`;
  `CARGO_PROFILE_RELEASE_DEBUG=1`). Copies of the binary and traces go under `target/profiling`, not `/tmp`.
- **`service_speed`.** Its `echo#Data` row is the held service call. The expression rows of `js_speed` are run under
  `small-oxide`.
- **The heap.** The peak of each process is in the stats file of a `./moq test-apps` run, under each engine.
- **The Mantle operations.** These are measured with `../oxide-bench/mantle-pg/` (its README), and the table is
  Round 32's.

### Next: the bindings' speed and the heap

Round 32 measured where the time goes. Plain JavaScript is as fast or faster on oxide. What is slower is the crossing
between the host and the engine.

**Rows that lose:**
- a host getter, `ec.user.nowTimestamp`: ×2.2;
- walking 100 context maps: ×3.3;
- decimals: ×1.3;
- cached entity finds: ×1.4;
- the trusted template: ×1.6;
- form-single screens: ×1.5 to ×1.6.

**The bindings allocate 4 to 10 times what QuickJS's do.** The fork is ours, so its embedding API can grow to fit what
the bindings need. Places to look first:
- the trampoline and argument conversion of a host call (`oxide/bind.rs`);
- the conversion of host maps and lists to JS and back (`oxide/convert.rs`, `entities.rs`): keep a value's wrapper
  rather than make a new one;
- the names exotic object's lookups (`oxide/names.rs`);
- the property-key cache;
- `entity_proxy`'s scan, which grows with a run's rows.

**The heap.** A test-apps worker peaks at 91 to 118 MB under `oxide`, against 126 to 140 MB under `rquickjs`. Two
causes remain, both in the fork:
- about 30 runtimes a process, each with its own inline caches and atoms (F1 step 3's sharing goes further);
- arenas that give back only their ends (a compacting sweep, or arenas in chunks).

A fork change goes on a branch of its own, gets a review, and runs the Sweep in a quiet window before it merges to
the fork's `main` (`../oxide-bench/README.md`).

**Round 33 (Profiling) is done:** the bindings' changes that need no fork change.
- `nowTimestamp` is 67 → 41 µs a 100 reads (QuickJS 31).
- The entity-list walk is −16%.
- The screen rows are −2 to −3%.

**Round 34 (Profiling) is done:** the fork's heap cap headroom, keys interned without a copy, an in-place flush on
`is_ordinary_object`, host data read with no handle, and localized reads without the lend.
- Walk itemList is 188 → 127 µs (QuickJS 59); Single.qvt 806 → 661 (510); List.qvt 100 16210 → 14487 (12953).

**Round 35:** plain objects made and read in one step, strings in one copy, and a Release flush with a fingerprint and
no copy. Walk itemList is 88 µs, Single.qvt 564 and List.qvt 100 13964. The screen rows are ×1.08 to ×1.15 of
QuickJS, or below it. Next: the engine core for the qvt rows, cached finds, and the locked-down template.

**Measuring a round.** Each change of a round goes behind a temporary switch (`MOQUI_OXIDE_OFF`, a module of the
bindings), so one release build measures every change on its own. The switches come out before the commit. Use
`../oxide-bench/round33/run-variants.sh` and `table.py`. A round is proved locally (`js_speed`, a profile, the touched
unit tests and the fork's API tests), with no Sweep, Mantle run or full-suite run.

### Next: Phase 4 (rquickjs removed)

Before the removal:
- **The default.** Make `oxide` the default and run a round of the suites, the acceptance suites and the client
  suite under it.
- **Engine differences to settle.** Decide which of the "Engine differences" sections' items to fix and which to list
  in [Parity-Diffs](Parity-Diffs.md):
  - `using` declarations, which need the fork's parser;
  - `performance.now()`'s whole milliseconds;
  - `Error.captureStackTrace`;
  - stack and message texts;
  - a decimal's identity after a releasing flush.
- **Port what stays on rquickjs** to oxide runtimes of their own, as `moqui-llm`'s `ui/standalone.rs` does:
  - `screen/vue_check.rs`, the Vue compiler;
  - `tests/browser_libs.rs`;
  - `screen/tests/qvt.rs`.

  `js_speed`'s QuickJS half goes.

The removal:
- **Remove the QuickJS bindings and engine:**
  - `script/runtime.rs` and `script/runtime/`, including `ec_extra.rs`, `bind.rs` and `compiled.rs`'s
    `SHARED_BYTECODE`;
  - the QuickJS side of `walker.rs`, `actions.rs`, `template.rs` and `screen/generate.rs`;
  - the rquickjs uses in `script/oxide/` (the cross-engine flush and the nested QuickJS paths);
  - the public functions that hand out a `Ctx`.
- **Remove the setting and what only routing needed:**
  - `script_engine` itself;
  - `small-oxide`'s hardened runtime, if the small kinds then run on the whole runtime;
  - the static check's routing role (`check.rs`, the census), keeping only what the difference check or security
    still needs, or removing it with the difference check.
- **Tests.** The 26 tests pinned with `quickjs_for_test` are deleted, or ported where they check behaviour a user
  sees. `on_both_engines` becomes a single run.
- **Dependencies and builds:**
  - `rquickjs` leaves the workspace's `Cargo.toml`, moqui-core (two entries, one with `bindgen`) and moqui-llm.
  - The browser build loses `bindgen` and the emsdk sysroot's QuickJS needs (`scripts/wasm-build.sh`); it keeps
    `native_depth_limit`.
  - `deny.toml` and the license review lose QuickJS's rows.
- **Docs:** Build-Test, Run-Deploy, `docs/interfaces/Web-Scripts.md` and Parity-Diffs.
- **The gate (§7):** `./moq test-all`, the browser build and the packages pass with no C in the script engine's build.
