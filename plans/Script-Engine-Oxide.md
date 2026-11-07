# Script engine: quickjs-oxide beside rquickjs

Status 2026-11-07: plan, nothing built. Server JavaScript runs on QuickJS through rquickjs (`crates/moqui-core/src/script`).
This plan adds [quickjs-oxide](https://github.com/pocket-nexus/quickjs-oxide), a QuickJS-compatible engine in safe Rust,
first for expressions and other small scripts, then for everything, and then removes rquickjs. It also changes our fork
of quickjs-oxide so compiled code is shared by all threads of the process, as compiled Groovy was in Java. The
measurements it rests on are in §1; the setting that picks the engine is §2; the fork's four changes are §6; the phases
and their gates are §7.

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

## 8. Testing

- **Unit tests for the bindings.** The tests in `script/` that check binding behaviour (conversions, tracked values,
  nested runs, errors) run under each engine from one body: a test helper sets the engine for the test's thread.
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
