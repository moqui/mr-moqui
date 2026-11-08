# Script engine: ideas from zipp.org for quickjs-oxide

Status 2026-11-08: ideas only, nothing built. The work starts after Phase 4 of
[Script-Engine-Oxide](Script-Engine-Oxide.md) (rquickjs removed) and the heap work going on now. Each item becomes a
[Profiling](Profiling.md) round, and a branch of the fork for the fork's items.

This plan lists what to take from [zipp.org](https://www.zipp.org/) into our fork of quickjs-oxide and into mr-moqui.
§1 says why ideas and not the engine, §2 gives the rows the work aims at, §3 the items in tiers, §4 what is not taken,
§5 how the work goes.

## 1. Why ideas, not the engine

zipp.org (`f2i-com/zipp.org`, v0.0.33, Apache-2.0, with Unicode-3.0 for its CLDR and Unicode-name data) is a
JavaScript engine in Rust: a register VM, NaN-boxed values, inline caches and an x86-64 JIT. It keeps a ledger of what
each change gained or lost, failures included (`docs/archive/PERF_LEDGER-B001-B252.md`, `docs/PERF_ROADMAP.md`, the
release notes and commit messages). In this plan, a B-number is an entry of that ledger and a hex id is a zipp commit.

**Why we do not use the engine itself:**
- **The embedding API.** The only public module is `zipp_vm::embed`. It runs one sandboxed script:
  - no host classes, exotic objects or function handles;
  - no re-entry from a host function;
  - one host channel, `__zippHostCall(kind, ...strings) -> string`.
- **Server use:**
  - No compiled code shared between engines.
  - A shape table per thread that never shrinks.
  - A panic in a JIT helper aborts the process.
  - Deadlines and heap caps need metering, which turns its ARM64 JIT off (that JIT is 839 lines anyway).
  - macOS is not a supported platform.
  - Its own figures show 2 to 6.6 times QuickJS's peak memory on bundled programs.
- **The project.** It is five months old, has one maintainer, is mostly agent-written, and its automatic CI has been
  paused since 2026-09-12.

**Why its ideas transfer.** Our fork's core is built the same way:
- NaN-boxed `u64` values and 32-bit heap indices that never move;
- a register machine with 8-byte instructions;
- shapes, and inline caches that go from mono to poly to a shared megamorphic table.

So its interpreter lessons apply more directly than they would to C QuickJS. It also means its largest wins over C
QuickJS, inline caches and shapes, are ones the fork already has.

**License.** We reimplement the ideas and copy no code. Our code stays MIT and can be proposed upstream. A commit
message names the B-number or commit an idea came from.

## 2. The rows the work aims at

Round 48 unless noted; µs a call.

| Row | QuickJS | oxide | Ratio | Cause, as recorded |
|---|---:|---:|---:|---|
| fancy, locked down (a snapshot) | 69.7 | 81.0 (824 allocations) | 1.16 | a snapshot's copy (Round 48); the interpreter on the template's own code (Round 43) |
| walk itemList, 100 context maps | 58.6 | 68.2 | 1.16 | a host map's conversion |
| 10 cached entity finds | 43.9 | 50.7 | 1.16 | a find's arguments |
| Labels, html | 226.2 | 248.7 | 1.10 | the bindings and the screen macros |
| Single html / Single.qvt | 317.5 / 514.1 | 340.1 / 546.5 | 1.07 / 1.06 | the screen macros |
| Expression from new text | 0.55 | 1.23 | 2.24 | compiling. Round 31's figure, not measured since the fork cut a short compile's allocations from 114 to 81 |
| HTML escape (regular expression) | 313 | 452 | 1.45 | Script-Engine-Oxide §1, not measured since F4 |
| Engine start and floor | 87, 164 KB | 201, 400 KB before F4; 172 KB after; 1 to 1.8 MB in Moqui | — | the realm built eagerly, and the bindings' start |
| Peak script heap of a worker | 114 to 153 MB | 103 to 118 MB | gate about 73 MB, missed | about 30 runtimes a process; arenas that give back only their ends |

- **Plain JavaScript is already faster on oxide,** at ×0.67 to ×0.96 of QuickJS: concatenation is 96 against 143.5,
  JSON 94 against 107.8.
- **zipp does not help the ×1.16 rows.** They sit at the boundary between host and engine, and zipp's own host crossing
  is JSON text.

Two facts in mr-moqui point the work:
- Generated templates build their output with `$o.push(...)` and `return $o.join("")` (`script/template.rs:683`,
  `1026`, `1030`).
- They escape through `$esc` (`template.rs:652`), a regular-expression `replace` whose callback receives a
  one-character string for each match.

## 3. The items

### Tier 0: measure first (mr-moqui)

Add rows to `js_speed`'s script section:
- `$esc` over 100 strings, most with nothing to escape;
- `JSON.parse` of the locked template's snapshot;
- 1000 numbers to text;
- engine start as a row, with time and bytes (today it is a note, `js_speed.rs:625`).

After Phase 4 these rows compare oxide before and after each change. Round 48 and Script-Engine-Oxide §1 give the
QuickJS reference.

### Tier 1: cheap, with clear evidence

1. **Release settings for the engine crates (mr-moqui `.cargo/config.toml`).**
   - `[profile.release]` sets only `panic`, so the engine builds with 16 codegen units and no fat LTO.
   - Upstream quickjs-oxide measures with fat LTO and one codegen unit (the fork's `Cargo.toml:96-98`); zipp records
     about 2% from that alone (its `Cargo.toml:58-60`).
   - Try `codegen-units = 1` for `quickjs-oxide`, `oxide-core` and `oxide-regexp`, then thin or fat LTO for the release
     package. Measure the rows and the build time.
2. **Numbers to text without a format-and-parse loop (fork, `core/src/vm/radix.rs:311`).**
   - Today `decimal_sequence` loops `format!("{:.*e}")` and a parse from 17 digits down: two or three `String`s a
     number, which JSON then encodes again.
   - In its place:
     - a path for short decimals (N/10^d, no formatter);
     - one shortest `{:e}` into a stack buffer, keeping QuickJS's rule at powers of two;
     - output written into the caller's buffer.
   - Evidence: zipp 0d2445f3, 1.08G to 0.70G instructions over 300,000 doubles.
   - Check: the outputs compared with today's over random doubles.
3. **One-character strings without an allocation (fork).**
   - A per-runtime table of the single-unit Latin-1 strings and `""`, filled as they are first asked for, so a
     runtime's floor does not grow.
   - Used by matches and captures, `charAt` and `[i]`, `split("")`, `String.fromCharCode` and JSON.
   - Evidence: zipp B62 (JSON −6.7%). Round 46 measured short concatenations, not these.
4. **`replace` with a callback (fork).**
   - A subject with no match is returned as it is.
   - A one-character match comes from item 3.
   - Read 7cf1f530's `replace_pristine` first, since Round 43 changed it.
   - Rows: `$esc` and the locked template.
5. **The rope threshold (fork, `core/src/vm/ops.rs:139`).**
   - `ROPE_MIN` is 64; try 128 and 256.
   - Evidence: zipp's sweep from 24 to 256 units took regex-log-scan −25% and markdown −10%. At 2048, one case was 8
     times slower.
6. **The template generator (mr-moqui, `template.rs`).**
   - Push one concatenation for each straight run of output, `$o.push(a + b + …)`, instead of one push per piece. The
     fork's `ConcatN` joins it in one allocation.
   - Evidence: zipp's joined concatenation chains (B124).
   - See whether the screen generators build output the same way.

### Tier 2: aimed at the rows that lose

7. **A one-byte string builder in the fork's core.**
   - It appends Latin-1 bytes until a wide unit arrives, then widens once.
   - Used by `Array.prototype.join` (the end of every template), `JSON.stringify`, `concat`, `repeat`, the `pad`
     methods and case conversion. Each of these builds a `Vec<u16>` today and narrows it with a scan
     (`StringRepr::from_units`, in the locked template's profile in Round 43).
   - Round 43 did this for `replace` only.
8. **`JSON.parse` (fork, `core/src/builtins/json.rs`).**
   - A Latin-1 source is scanned where it lies; `json.rs:701` widens it today.
   - **Key plans:** each sequence of member names maps to a plan, so an object is made once, at its final shape, with
     exactly its slots. zipp 57bbb00f: −11.9% instructions. A parse that keeps missing stops using them.
   - Integers of up to 15 digits are read into a `u64` (B261).
   - Row: the locked template, which parses its snapshot on every render.
9. **`JSON.stringify` (fork).**
   - It walks plain objects and arrays directly, once it has proved that no `toJSON` can be reached.
   - Each shape's member slots and quoted keys are kept, and dropped at a collection.
   - Output goes through item 7 and numbers through item 2.
   - Evidence: zipp bb6eded2 and 43693b4e (−19% instructions).
10. **A native's `name` and `length` from the native table (fork, `core/src/builtins/mod.rs:906`).**
    - Today each native gets an interned name atom and two real properties, about 600 a realm.
    - Instead the natives share one pending shape (the `AUTOINIT` form that bytecode closures already use), and the
      values come from the native table until a script redefines them.
    - Rows: engine start, and each runtime's floor, which about 30 runtimes a process multiply into the heap gate.
    - Comes after the heap work going on now.
11. **The regular-expression executor (fork, `regexp/`).**
    - Instructions for a loop over one character, with one backtrack record for the loop instead of a `Split` frame
      per character.
    - Loops made possessive where the next item cannot match what the loop matched (zipp B116: −98.9% backtrack
      pushes).
    - Lazy loops that read ahead only as far as they need.
    - A scan for a literal or a set of characters over a Latin-1 subject, beside `FirstUnits`, with no `memchr`
      dependency.
    - Gates: upstream's RegExp runs, and the 100 RegExp negative-diagnostic contracts.

### Tier 3: later

12. **Profile-guided builds of the release packages.** zipp B114: −13% overall, and −10 to −30% on rows the
    interpreter dominates.
13. **The dispatch loop, after a new profile of the qvt rows:**
    - instructions joined in pairs (zipp's FusedCode, −5%; its triples lost 9 to 20% elsewhere through code layout);
    - calls to leaf functions without a frame, each fast path keeping its collection safe point (B117);
    - `typeof x === "lit"` as one instruction (B54);
    - `x == null` without reading the heap.

    All within the fork's loop size gate.
14. **`using` and `await using`, a Phase 4 engine difference.** zipp's grammar (`parse/stmt.rs:732-810`,
    `parser.rs:593`) and its DisposeResources (`construct/modules_dispose.rs`) are the reference for our parser and
    lowering.
15. **Generational collection** (the fork's `docs/planned.md` §6). zipp's `docs/archive/NURSERY_DESIGN.md` is reading
    for it: sticky mark bits, the grain of the remembered set, and the pitfalls. Also a collection schedule that
    counts payload bytes.
16. **A JIT, a distant option.** Our values and instructions suit one. zipp's x86-64 JIT shows:
    - deadlines through step loans per basic block;
    - counters that cost 4 to 12% with nothing compiled;
    - `MAP_JIT` and W^X on macOS, and no JIT on iOS;
    - ARM64 as a backend of its own.

## 4. Not taken

| Idea | Why not |
|---|---|
| zipp's regular-expression engine (a fork of regress) in place of ours | Loses QuickJS's exact errors, the interrupt, fallible allocation, Latin-1 matching and having no dependencies |
| WTF-8 strings | Against our strings, which index by UTF-16 unit |
| zipp's object layout | Its memory |
| Paged or chunked arenas | Measured in Round 44: 10 to 15% back, for an indirection on every access. zipp does not give back interior free space either |
| A thread that frees large payloads | Returns nothing to the system, and needs threads |
| Small strings stored inline | Round 46: 2% at most |
| zipp's `with` and direct `eval` | Our front end already has the design; what is missing is our lowering |
| A cache of `"prefix" + int` | Small effect on our rows |
| Python | Not a JavaScript engine's concern |

## 5. How the work goes

- **One item at a time:** a short design note, a branch, and a review of the diff before it is measured or merged.
- **A fork item:**
  - It is a branch from upstream's `main`, measured from mr-moqui behind a temporary switch, as Rounds 33 to 47 were.
  - Before it merges to the fork's `main`, upstream's Sweep and instruction gate run on the Linux machine, since they
    read `instructions:u` through `perf`.
  - mr-moqui's pin then moves, with a Profiling round.
- **Working on a Mac:** the fork is cloned to `../quickjs-oxide` and patched in through an uncommitted `[patch]`, as
  Script-Engine-Oxide §6 describes.
- **Checks for each item:**
  - `js_speed`: two runs each in a release build with nothing else building, compared at the better run.
  - A fork item: the fork's tests and the test262 directories of its area.
  - An mr-moqui item: the specs it touched, and `./moq test-framework` before a commit.
