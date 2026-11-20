# Mantle USL execution-method proof plan

Status: proposed full-service experiments, not an execution-architecture decision. **Executed source-subset benchmarks and raw results:** [experiments/mantle-execution/README.md](../experiments/mantle-execution/README.md). These prove extracted arithmetic/bridge costs and a reduced durable database path, not full Mantle business throughput.

References: [source audit](MantleUSL-Performance-Audit.md), [architecture](archive/Architecture-Proposal.md), [context contract](archive/ContextObjects.md), [implementation order](archive/Implementation-Phases.md), [type generation](archive/TypeGeneration.md), [deployment/startup requirements](Performance.md).

## 1. Questions and claim boundaries

Prove with real Mantle source:

1. How much CPU, allocation, memory, and worker occupancy does each execution method require?
2. Which method minimizes the end-to-end cost of a completed, durable business transaction?
3. Which method preserves Mantle semantics and supports its dynamic features?
4. What authoring/rebuild/reload/startup costs buy those improvements?
5. Can the selected deployment sustain **100,000 completed durable business transactions/s** for each explicitly specified workload profile?

This document measures business transactions, not single-row writes. `Performance.md` currently describes a separate single-row target; its result cannot establish the business target here. Report business completions, database commits, and row changes separately. Nested `force-new` commits do not count as extra business completions.

Business-resource contention is excluded from the capacity workloads: no competing root requests mutate the same business resource. Required sequential work and repeated updates inside one request remain. Shared indexes, WAL, caches, engine locks, storage and connection/worker limits remain in scope. Use fixed hardware per paired comparison; any scale-out result identifies its own topology and resources.

A method does not pass because it accepts requests or durably enqueues jobs. It passes when the defined business completion condition is met and its required commits succeeded. Optional external stages and asynchronous projections have explicit completion/freshness definitions. Do not move required synchronous accounting or validation behind the acknowledgment.

## 2. Freeze the evidence and semantic contract

Before implementation, produce a versioned run manifest:

- Exact framework, mr-mantle, and configured integration-component commits, tracked/untracked changes and source hashes. The audit used the Java mantle-usl `8503eeda6bfb7ee8c3207a4fda61487fc42895a7`; the benchmark chooses and pins its actual revision rather than assuming all installed copies match.
- Original Groovy XML/scripts, migrated JS XML/scripts, generated code, and a reviewed mapping of changed expressions. Include the external `PartyServices/findParty.groovy`, ECA bodies and configured hook implementations.
- Database schema/indexes, seed/install data, settings, service/ECA/auth/audit/feed configuration, fixture-generator seed and initial database snapshot hash.
- Hardware: physical/logical CPUs, NUMA, memory, storage/filesystem, flush assurances, OS/kernel, container limits, network, proxy/TLS, power policy, competing load and topology.
- Compiler/engine/driver versions, release profile, optimization/LTO settings, enabled features, code-cache policy, workers, pool maximum/reserve/parking, queue limits, timeouts and readiness requirements.

Classify the full service inventory: transactional writer; transactional reader; report/search; batch/file conversion; external integration; interface/unsupported. Performance samples do not prove all service definitions. Extend semantic coverage toward all applicable Mantle tests/definitions, and report tested/untested/unsupported lists, not a blanket compatibility claim.

Java Moqui is the behavior oracle for fixtures, not an engine-only speed control: its drivers and framework implementation differ. Use existing Mantle business-flow tests where applicable. Record intentional migration differences explicitly; all compared Rust methods implement the same agreed contract. No backend-specific exemptions for a failed case.

## 3. Methods to compare

All methods enter the same service facade and use the same entity/transaction/auth/ECA/audit/feed implementations. Execution adapters belong to the experiment, not a new production interface commitment. Do not build multiple complete frameworks.

| ID | Method | Performance hypothesis | Main problems to prove or falsify |
|---|---|---|---|
| Q | Whole-service generated QuickJS | Few engine entries, cached compilation, script-like reload, small runtime | Interpreted loops, bare-name proxies, host decimal/property calls, GC and per-worker caches |
| QH | Same QuickJS with coarse native helpers | Removes frequent crossings/loops while retaining authoring behavior | Copy/serialization overhead, helper granularity, immediate context/alias visibility, rounding parity |
| V | Same authored/generated JS on V8 | JIT can improve sustained JS-heavy loops | Warmup/deoptimization, host-call floor, per-worker memory/startup, binding parity, interrupts and GC tails |
| I | Predecoded Rust XML/action executor, JS expressions/scripts | Direct native structural actions without rustc on edits | Thousands of small expression-engine entries; synchronization/boxing can exceed whole-service JS cost |
| A | XML/action IR to Rust AOT, explicit JS fallback | Native control flow and direct host/decimal operations in supported regions | Compiler coverage, typed-local/context coherence, callback reentrancy, fallback overhead, rebuild/artifact invalidation |
| R | Reviewed hand-native implementation of the same selected Mantle service | Native orchestration reference without interpreter overhead | Manual implementation drift, maintenance cost; cannot be sold as automatic XML compilation |

R must be a line/behavior-mapped implementation of the selected service, not a simpler substitute. QH/A/R may not bypass facade policy or silently change queries, transaction boundaries or hooks. V is an experimental alternative, not a second engine planned for production.

For A, declare the supported subset. Unsupported scripts stay explicitly in JS or produce a compile-time unsupported diagnostic. Report native coverage, fallback counts and time per workload; a native leaf helper does not establish whole-service AOT performance.

### Secondary candidates

New DSL/Rhai/Lua, WASM, and a custom Cranelift/LLVM JIT are separate feasibility tracks, not prerequisites for the six-method comparison. Each must implement the same selected Mantle behavior before receiving throughput numbers. Use the same metrics and tests, plus:

- DSL/Rhai/Lua: migration edits, dynamic/decimal/context compatibility and runtime crossing costs.
- WASM: chosen frontend/semantics, handle/import costs, memory copies, fuel interruption, and actual isolation limits. Compiling R to WASM proves that mapped native service, not arbitrary JS/Groovy support.
- Custom JIT: frontend coverage, compile latency, code memory, safepoints and invalidation/lifetime behavior.

An infeasible/deferred candidate is labeled **not measured**, with its blocker. Do not manufacture estimates or claim it is slower without an implementation.

## 4. Real-service workload matrix

Extract complete real service bodies and their dependencies; migrate expressions without simplifying business work. Use the pinned component's actual parameters. Each fixture declares input, configuration, expected outputs, required durable state and background effects.

| Profile | Real entry points / dependency paths | Dimensions and reason |
|---|---|---|
| P1: small entity orchestration | `mantle.party.PartyServices.create#Person` | Independent parties; configured role/classification paths. Establish dispatch/context/policy overhead on short services. |
| P2: order import | `mantle.order.OrderBulkServices.store#Order` | Create and update separately; 1/10/100 items, parties and notes; preserve external revisions, item removal and totals. Exposes per-item recalculation and matching costs. |
| P3: order placement/approval | `OrderServices.place#Order`, `autoApprove#Order`, `OrderInfoServices.check#OrderPreApprove` | Named store settings, payments/addresses, approval hooks and line counts. Do not combine all calls into a new transaction if the real client flow commits them separately. |
| P4: invoice finalization/posting | invoice status update through the service facade, `InvoiceAutoPostServices.post#Invoice`, `LedgerServices.post#AcctgTrans` | 1/10/100 items; one/two accounting organizations, entries, periods; real-time summaries on/off as separate configurations. Include total checks/payment application and ledger hooks. |
| P5: inventory reservation | `mantle.product.AssetServices.reserve#AssetForOrderItem` | Independent eligible assets per request; 1/10/100 candidates and selected counts separately; BOM depth/breadth; histories of 0/100/10,000 details. Exposes queries, recursion and historical sums without hot-resource waiting. |
| P6: payment application | `PaymentServices`' actual apply services and entity-auto PaymentApplication calls | Payment-to-invoice and invoice-to-invoice paths; application counts; verify both-side totals, posting/status effects, rounding and reverse/cancel behavior. Pin exact selected service names before running. |
| P7: arithmetic/collection services | `get#OrderItemTotal`, `get#InvoiceTotal`, `calculate#AcctgTransTrialBalance`, pricing and tax preparation | Use real service logic and representative input lists. Separates decimal/collection/bridge costs; read-only functions are not counted toward the durable-write target. |
| P8: secondary/batch work | order display/contact info, payroll/file generation, duplicate checks, labels | Measures JS-heavy code, hidden queries, external waits, async joins and memory. Report independently, not as transactional-writer capacity. |

Start with P1/P2/P4/P5/P6/P7, then expand. P3/P8 and dynamic integrations are mandatory compatibility coverage before any whole-Mantle suitability conclusion. Track largest realistic inputs separately from the standard capacity profiles.

Configuration includes enabled ECAs, real auth rules/users, audit/feed behavior, business reference/history sizes and integrations. Do not cherry-pick early-return/no-accounting/no-inventory branches as the headline. Report light/heavy configurations side by side. Standard capacity data is independent; self-dependent account/period work still executes. If independent fixtures are impossible for a specific profile, mark that profile ineligible for the noncontending benchmark rather than suppressing its required hooks.

External APIs: use deterministic protocol-faithful local endpoints for repeatable timing and failure tests, and separately report actual integration tests. A no-op mock is not a real payment/carrier transaction. Report provider latency separately; do not infer external-provider capacity from a local endpoint.

## 5. Two comparisons: language cost and business optimization

Keep two workload tracks for every method:

- **S0: faithful migrated baseline.** Preserve current recalculation, loops, query order and immediate hook behavior. This reveals what the engine alone changes.
- **S1: reviewed optimized behavior.** Incremental totals, indexed grouping/matching, reused reference/entry data and approved batching. Apply the same algorithmic improvements across methods and rerun parity tests.

First do engine-only comparisons with matching SQL/callback traces and boundaries. If a helper/native backend intentionally changes query shape or algorithm, label the result combined optimization rather than interpreter speedup. Then compare full production candidates, with each permitted optimization documented.

Do not compare Q/S0 against A/S1 and attribute the whole improvement to Rust. Preserve observable mid-transaction totals, status transitions, audit/revision/feed effects; if an optimization intentionally changes them, it belongs to a separately specified business contract and cannot win the same-contract comparison.

## 6. Correctness gate before performance

A backend/profile proceeds only after matching observable results and durable state. Use controlled clocks/ID sources where possible; otherwise canonicalize nondeterministic values without discarding required relationships or audits.

Required cases drawn from the audit:

- Missing versus null, false, zero, empty strings/maps/lists; short-circuiting, Elvis/default behavior, numeric/string equality, decimal truthiness and membership.
- Negative/zero/fractional amounts, currency rounding ties, per-line versus final rounding, scales, division by zero, overflow and values outside `rust_decimal` capacity. Unsupported ranges must fail consistently, not silently narrow.
- Dynamic context keys, nested scope shadowing, merge/replace outputs, result-map propagation and aliases mutated by scripts/nested calls. A native local must observe a later context change.
- Entity/list aliasing, clone/refresh, related reads, dynamic views, caches, transaction-cache/sim behavior and `no-tx-cache` paths. Diagnostic sim runs are not durable-write results.
- Literal service overrides, configured hook names, changed definition generations, external script bodies and missing integrations. Never permit a missing gateway implementation to silently turn the business workload into a no-op.
- Default joins, ignore, `force-new`, rollback-only, savepoints where required, failed commit, partial success after later failure, `ignore-error`, messages versus thrown exceptions and early return.
- ECA before/after timing, original values, set-results, reentrant guards, commit/rollback callbacks, pending feed durability and dependency ordering.
- Recursion, break/continue, script try/catch/finally, iterator closing, context/transaction cleanup and injected host errors/panics. Native panic handling must not replace structured business errors.
- CPU-loop deadline, blocked database cancellation, memory exhaustion and budget enforcement. Native code must have explicit safepoints/isolation policy; memory caps are not automatically equivalent to a JS heap limit.
- Async submit/full queue/join from a worker; separate contexts and runtimes, no borrowed/JS values crossing threads, scheduler retries and bounded overflow.
- Reload during active requests/callbacks, stale AOT metadata/code, invalid artifacts, failed rebuild and service override invalidation. Pin in-flight code; no unsafe native unloading.

Use existing business-flow tests, focused fixtures, property tests for generated-code escaping and financial boundary cases, and failure injection. Report pass/fail/unsupported per method. This list guides coverage; only executed tests establish coverage, not a promise that all edge cases have been discovered.

## 7. Three measurement layers

### L1: execution cost, not durable throughput

Run extracted real service computations with representative already-materialized host values. Include the actual context/decimal/entity bindings and mutation visibility. Where SQL-dependent orchestration is replayed, label the deterministic driver-record/replay or in-memory fixture clearly; use it only to isolate runtime costs. Do not present replay results as transaction throughput or proof of full facade behavior.

Measure cached function execution, host crossings by category, expression-engine entries, allocations/bytes, GC events/time, CPU and wall time, and object-copy volumes. Verify outputs/side effects; prevent dead-code elimination. Separate per-call from per-item results. Run stable and polymorphic input variants to expose JIT/deoptimization and dynamic branches.

### L2: real facade and durable database

Call the selected service through the shared facades against a real PostgreSQL database. Use release builds and live SQL. Required durability: `fsync=on`, `synchronous_commit=on`, `full_page_writes=on`; logged business tables; storage documented to honor flushes. Capture actual settings including WAL synchronization and replication guarantees. No transaction batching across independent business requests and no early acknowledgment.

Measure actual root business completion, all nested commits, statements/queries, rows/index work where observable, WAL bytes, worker occupancy, pool wait/checkout testing, SQL wait, commit wait and cleanup. Verify resulting state. Preload identical data/caches for paired warm runs; report cold runs separately. No `UNLOGGED`/RAM-only database substituted for the durable headline.

### L3: authenticated deployment end to end

Drive actual HTTP/RPC endpoints with connection reuse and the configured proxy/TLS. Requests follow the chosen client business flow, not a benchmark-only simplified endpoint. Include auth, parameter decoding/validation, routing, response encoding and durable success acknowledgment.

Run one-node and maximum-scale setups separately. A distributed run includes networking, replication/freshness and processing/batch consumers; same business semantics. Load generators run outside the measured one-node resource budget, but their CPU/network limitations are recorded.

## 8. Load protocol and statistical credibility

1. Validate one deterministic transaction and its complete state before load.
2. Run single-worker samples to establish CPU/crossing counts, then concurrency sweeps (for example 1/4/16/32/64 and upward where pool rules/resources permit). Enforce reserve/parking startup constraints; never silently exceed them.
3. Run separate cold-start, first-use, and warm-cache measurements. For V8, report ramp time and stable warmed throughput; include deoptimization/GC variation. For every engine, distinguish compiled-code warmth from DB data/statement-cache warmth.
4. Use closed-loop sweeps to locate saturation, then open-loop offered-rate tests around it and at 100k/s. Record intended arrival, actual dispatch, admission, service start, durable completion and response. Report generator lateness, all queues/rejections/timeouts; avoid coordinated-omission claims based only on closed-loop latency.
5. Exploration runs may be short and are labeled diagnostic. Capacity candidates get at least five independent repetitions, with method order randomized/rotated and identical restored data. Use per-run statistics, not millions of correlated requests as independent samples.
6. Capacity runs last at least 15 minutes after a reported warmup and span at least three configured checkpoint cycles; extend if checkpoint/storage/GC behavior has not stabilized. The winner gets a one-hour soak. Monitor table growth, vacuum/bloat, WAL/storage headroom and thermal/power behavior.
7. Restore or regenerate equivalent fixtures between runs. Preseed independent key ranges and business ownership; avoid benchmark key-allocation overhead becoming an undocumented competing system. IDs and initial state must still preserve actual service semantics.
8. Compare equal fixed resources first. Separately report tuning sweeps and best-achievable configurations with all changes. Do not disguise a larger machine/pool as an engine win.

Headline rate includes every successfully completed request in the declared measurement window, with late/in-flight completions reported separately. Report per-run rates and confidence intervals across runs; show p50/p95/p99/p99.9 and worst observed latency where sample counts support them. Select an acceptable p99/admission/rejection/freshness SLO before running; 100k is not met by an ever-growing queue.

## 9. Metrics and attribution

| Group | Required measurements |
|---|---|
| Business | Offered/admitted/completed/s, root success/error/timeout/retry counts, ambiguous outcomes, state mismatches, business latency histogram |
| Transactions | Commit/rollback/s, nested independent commits per root, statements/rows per root, durable completion versus response timestamps |
| Runtime | Application CPU/root and per-item, engine entries, host calls by category, proxies/context accesses where instrumentable, allocation/copy bytes, GC and peak/steady RSS/code cache |
| Facades | Validation/auth/ECA counts and CPU, service depth/dispatch, context cleanup, cache hit/miss/invalidation, iterator lifetime |
| Workers/pools | Occupied workers, queue depth/time, active/parked/reserve connections, acquisition/test costs, max usage and failures |
| PostgreSQL | Server CPU, SQL timing/plans in diagnostic runs, WAL bytes/records/FPI, WAL/data I/O, flush/checkpoint/vacuum activity, locks and waits, storage latency/utilization |
| Background | Feed/job/outbox backlog slope, oldest-item age, consumer completion/error/retry/s, projection freshness, configured notification/email load |
| UX | XML parse/resolution/codegen time, compile/link time, per-worker engine/function initialization, edit-to-usable-service, cold readiness, invalidation/rebuild failures |

Use monotonic timestamps. Separate exclusive CPU from inclusive nested spans: do not sum overlapping parent/child service CPU or label wall-time blocked on SQL as JS CPU. Account for application, proxy, database, background and load-generator processes separately. Attribute GC/runtime background CPU too.

Use interval deltas and isolated clusters for WAL/I/O statistics; record setup/background activity rather than casually subtracting an unrelated baseline. PostgreSQL 18 WAL I/O is in `pg_stat_io`, alongside applicable `pg_stat_wal` counters. Verify actual version/schema. Transactions divided by fsync counts is only a diagnostic ratio, not exact group-commit size; exact barrier/group claims require suitable instrumentation.

Detailed tracing/perf/syscall sampling is for diagnostic runs. Headline runs use low-overhead counters; quantify instrumentation overhead in matched runs. Do not stack heavy profiling and present its throughput as the uninstrumented production ceiling.

For helpers/AOT/native code, report what remained dynamic and how many objects were copied. For read-model/processing separation, measure network and freshness/backlog rather than claiming the work disappeared.

## 10. Durability and sustained-state checks

For each deterministic fixture validate invoices/orders/payments/asset quantities, ledger debit-credit balance, statuses, revisions, configured audit rows and pending feed state. Check both short correctness runs and sampled/full aggregate invariants under load. Row counts alone are insufficient.

Track request IDs and acknowledged outcomes externally. After a successful durable response, terminate/restart application and separately database processes; verify all acknowledged transactions still satisfy expected committed invariants. Include retries and crash-after-commit-before-response cases: lack of response can be ambiguous and must not be treated as a guaranteed rollback or double-counted completion. `force-new` partial successes follow their specified contract.

Process-kill tests demonstrate crash-recovery behavior, not physical power-loss protection. Hardware flush compliance/power-loss assurances remain documented dependencies; test actual power-loss behavior only in an approved isolated environment.

Background work must run during sustained capacity tests, reach a stable backlog/freshness bound, and drain successfully. Work separated across nodes still counts toward total resources. Required derived updates are not silently dropped to protect the measured writer.

## 11. Development/startup experiment

Use the actual corpus, not a loop of one service repeated as fake definitions. Report loaded bytes/nodes/services and dependency graph.

Measure new process through authenticated DB-backed readiness, per `Performance.md`, separately for existing/empty DB and cold/warm files. Include enabled child-process startup, schema/seed work and required engine initialization. Missing compiler artifacts either fail explicitly or their generation counts when required for readiness.

For Q/QH/V/I, measure edit/invalidation/load/compile and first request. For A/R, measure clean versus incremental generation/build/link/replacement and first request, including failed builds. Production AOT compilation may occur before deployment; label it a build cost, not an artificially excluded launch step when the measured launch itself performs compilation. The 500 ms target does not include optional development rebuilds, but cannot exclude work required by the actual deployment launch.

Reload cases: changed expression, changed service signature, entity extension, configured implementation, ECA, shared helper/macro and dependency cascade. Verify old in-flight work uses its pinned version and new requests use the new one. Prefer process replacement for native code; design embedded-DB ownership/handoff explicitly.

## 12. Stages, stop/go gates and deliverables

### Stage A — Inventory and oracle

Freeze manifests, classify definitions, choose exact entry points/configurations, implement fixtures, and run Java parity oracles. Deliver source/coverage inventory, deterministic snapshots, expected outcomes and migration-difference ledger. Do not start six parallel implementations of an unfrozen contract.

### Stage B — Minimum real QuickJS path

Complete the actual shared facade/transaction/ECA path needed for Q. Execute P1/P2/P4/P5/P6/P7 correctly through the real generated code and PostgreSQL. No fake database host for L2/L3. Deliver semantic suite, command traces, durable-state checks and Q/S0 metrics.

### Stage C — Least-expensive improvement

Implement S1 work-shape fixes with approved semantics and QH helpers. Run the same corpus/parity suite and paired Q/QH measurements. Deliver crossing/allocation/CPU reductions, SQL/callback trace comparison, and all algorithm changes. If SQL/commit still dominate, do not claim a new engine will solve them.

### Stage D — Execution-method comparison

Implement R reference services; reuse bindings/fixtures for V; prototype I and A on the same selected services. Start with P7/L1, but require complete P2/P4/P5/P6 paths before choosing a method for business throughput. Report unsupported coverage and fallback costs. Stop an approach with correctness failures or unjustified compiler complexity; record the blocker, not fictional throughput.

### Stage E — Capacity and UX

Run L2/L3 sweeps, repeated sustained tests, durability/failure tests, deployment topology comparisons and startup/reload experiments. Deliver complete raw evidence and the one-hour finalist soak.

Store reproducible scripts/fixture definitions in version control; store large raw traces/database snapshots outside the source tree with checksums and an index. Each result identifies the exact executable artifact and run manifest. Never include secrets/credentials in captures.

### Required comparison table

Populate this per profile, method and S0/S1 variant; empty cells mean not measured:

```text
Method / native coverage / semantic pass rate / hardware+configuration
Business completions/s; commits/root; rows/root; SQL/root; WAL bytes/root
Application CPU us/root; database CPU us/root; worker occupancy us/root
Host crossings/root; allocation/copy bytes/root; GC; steady/peak RSS
p50/p95/p99/p99.9; failures/rejections/ambiguous outcomes; backlog/freshness
Cold readiness; first-use cost; clean/incremental build; edit-to-ready
Run repetitions/duration/confidence interval; raw evidence location
```

### Selection rule

1. Disqualify incorrect behavior, lost durable acknowledgments, unacceptable failure/latency/freshness, or unsupported required integrations.
2. Determine the limiting resource from measured CPU, occupancy, SQL, WAL/I/O and backlog. At 100k/s, 100 us application CPU/root consumes ten fully occupied cores; concurrency needs `100,000 × mean occupancy seconds`. These are necessary budgets, not predicted rates.
3. Compare paired same-resource runs and report uncertainty. Predeclare a materiality threshold (suggested: >=10% end-to-end throughput improvement or >=20% application CPU reduction at the same achieved rate), alongside memory/UX costs. These are decision thresholds, not expected results; require sufficient evidence rather than a single lucky run.
4. Choose the simplest method meeting the declared capacity/SLO/coverage with resource headroom. If none meets 100k, report the measured ceiling and limiting resources. A larger cluster is a new deployment result, not retroactive proof for one node.
5. State the exact tested profiles and bounds. A winner on 10-line orders does not prove 100-line invoices, arbitrary integrations, or every Mantle process. Publish all failed/deferred cases as well as the winner.

No performance method is accepted solely because it compiles to native code or wins an isolated arithmetic benchmark.
