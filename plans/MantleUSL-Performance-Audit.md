# Mantle USL performance audit

Execution-method experiments, correctness gates, metrics and acceptance criteria: [MantleUSL-Execution-Benchmark-Plan.md](MantleUSL-Execution-Benchmark-Plan.md). Executed source-subset measurements: [experiments/mantle-execution/README.md](../experiments/mantle-execution/README.md).

## Scope and verdict

Audited `/home/user/moqui/master/runtime/components/addon/mantle-usl`, commit `8503eeda6bfb7ee8c3207a4fda61487fc42895a7`, tracked working-tree source, 2026-10-13. No tracked modifications were present. Other installed copies have different revisions; this report is not a claim about all versions. Untracked conflict copies and build output were excluded.

Target: **100,000 durable commits / completed reads of transactional business data per second**, not row writes or business processes. The business-process-rate examples below are conditional illustrations; map each process to its actual commits/reads before comparing with the target. Independent business resources are assumed. Business lock contention is excluded. The SQL calls, arithmetic, index changes, WAL, framework policy, and callbacks required even without contention remain in scope.

**Finding:** the source has substantial avoidable work amplification before engine speed becomes the main question. The highest-priority issues are per-item total recalculation, synchronous ECA/service cascades, historical summary queries, and database round trips. QuickJS can remain the planned engine, but it is not yet proven adequate: financial arithmetic and collection-heavy paths make the Rust/JS bridge a real risk, not merely a theoretical one.

This is a static audit. No migrated JS runtime or Rust service implementation exists to profile. Rankings are engineering hypotheses backed by source paths, not measured percentages. XML element counts are code sites, not executed SQL counts, and branches/caches matter.

## Census

Parsed all 73 tracked service XML files and all 4 entity-ECA XML files without parse errors.

| Source characteristic | Count |
|---|---:|
| Service definitions | 1,044 |
| Inline services / interfaces / entity-auto / external script | 1,016 / 26 / 1 / 1 |
| Entity-find / find-one / find-count sites in service XML | 1,076 / 1,035 / 36 |
| Service-call sites in service XML, including SECAs | 1,721 |
| Iterate / while / script elements in service XML | 747 / 12 / 672 |
| Set / if elements in service XML | 4,661 / 3,531 |
| Service definitions with entity/service operations under XML loops | 293 |
| Such operation sites under XML loops | 867 |
| Script-body lines inside service action bodies | 5,689 |
| Explicit `ec.entity` / `ec.service` mentions in those scripts | 88 / 44 |
| SECAs: post-service / tx-commit / pre-validate | 53 / 6 / 1 |
| Entity ECAs | 19 |
| Service definitions with explicit direct self-calls | 21 |

Within service action bodies, 222 read sites explicitly request `cache="true"`; other sites may still use entity defaults or transaction-cache behavior. 1,039 definitions omit `transaction` or explicitly use `use-or-begin`, 3 use `cache`, 1 `force-new`, and 1 `ignore`. Those counts describe declarations, not independently committed transactions: nested default calls join their caller. There are also 9 XML call-site `force-new` overrides.

The scan resolves 975 direct call sites to locally defined services. Entity-auto, framework, dynamically configured, script-issued, and relationship-triggered work is additional. A call graph based only on `<service-call>` is incomplete.

Reproducer and complete inventory are retained outside the repository:

```text
/home/user/moqui/artifacts/mr-moqui-mantle-audit-20261013/
  audit-mantle.py   # Python + lxml; scans git-tracked source
  inventory.json   # per-service sites, source lines, calls, SECA/EECA inventory
  summary.txt
```

Source references below are relative to the audited component root.

## 1. Per-item recalculation: highest-priority amplification

### Orders

Trace:

```text
create/update OrderItem
  entity/Order.eecas.xml:16 — OrderItemHandleChange
  service/mantle/order/OrderServices.xml:2329 — handle#OrderItemChange
  :2523 — update#OrderPartTotal
      fetch all items for the part
      invoke get#OrderItemTotal for every item
      update part; check/update proposed/promised payment
  :2506 — update#OrderHeaderTotal
      fetch all parts; sum; update header
```

For ordinary new items, the change handler takes the totals path. Building N items individually in one part therefore visits approximately `1 + 2 + ... + N = N(N+1)/2` item records, absent a changed callback strategy. `get#OrderItemTotal` defaults child recursion off; this triangular cost does not depend on recursive child queries.

| Lines created | Total item-service invocations | At 100k documents/s |
|---:|---:|---:|
| 10 | 55 | 5.5 million invocations/s |
| 100 | 5,050 | 505 million invocations/s |

This excludes tax/promo child items and other work. Caches can reduce SQL, but cannot eliminate the repeated traversal, decimal arithmetic, validation/dispatch, or updates automatically.

Store orders can also execute `handle#OrderMajorChange` (`OrderServices.xml:2405`) after each significant non-promo/non-tax item change. It deletes/recreates promotion, shipping, and tax items and increments the order revision. Its shared-map guard prevents reentrant recursion; it does **not** combine separate sequential item changes into one calculation.

### Invoices and payment applications

`entity/Accounting.eecas.xml:16` invokes `update#InvoiceTotals` on each InvoiceItem create/update/delete. `InvoiceServices.xml:283` calls `get#InvoiceTotal` (`:245`), which fetches all invoice items and payment applications, calculates totals with per-item rounding, then updates the invoice. That invoice update also triggers due-date handling (`Accounting.eecas.xml:29`; `InvoiceServices.xml:717`).

Sequential invoice-item creation has the same triangular traversal pattern. PaymentApplication changes recalculate participating invoice totals and payment totals (`Accounting.eecas.xml:21,42`; `PaymentServices.xml:205`), potentially touching both sides of a transfer.

**Change:** implement incremental totals where correctness permits, preserving per-line rounding and original-value/delete behavior. For complex whole-document calculation, add an explicit bulk operation with well-defined intermediate visibility and final validation. Do not silently defer existing immediate ECAs: services can read the totals mid-transaction, and callbacks/status transitions depend on them. Any aggregation/coalescing requires parity tests for those observations, revisions, audit and feed behavior.

## 2. Synchronous service/ECA cascades multiply database work

Examples:

- `service/AccountingLedger.secas.xml:18`: finalizing/approving an invoice calls `InvoiceAutoPostServices.post#Invoice` synchronously at `post-service`.
- `service/AccountingInvoice.secas.xml:37,60`: the same status transition checks totals and applies matching payments, subject to conditions. It is not just one invoice UPDATE.
- `InvoiceAutoPostServices.xml:17`: reads items/preferences, creates accounting transactions for applicable parties, resolves accounts and creates entries per item, then calls `LedgerServices.post#AcctgTrans` (`:303,307`).
- `LedgerServices.xml:487,515`: posting fetches the entries, then `calculate#AcctgTransTrialBalance` fetches them again (`:461`). Optional real-time summaries add entry/account/period work (`:673`). This extra work remains costly even when no record is contested.
- `AccountingInvoice.secas.xml:16`: packing an outgoing shipment can synchronously call `InvoiceServices.create#SalesShipmentInvoices` (`:1063`), which has 37 entity/service operation sites under loops. These are branch sites, not 37 operations necessarily executed per row.

**Change:** reuse already available entries/items/preferences within a transaction using explicit inputs and version/overlay-aware caches. Batch independent reads and writes where action order, ECAs, returned IDs, and error semantics permit. Never turn required accounting posting into eventual background work merely to report a faster completion rate.

## 3. Historical summary scans: cost grows with data, not just request size

`entity/ProductAsset.eecas.xml:17` calls `AssetServices.update#AssetFromDetail` for every AssetDetail create. At `AssetServices.xml:434`, the service:

- reads the Asset;
- uses incremental differences if transaction-cache mode is active;
- otherwise queries `AssetDetailSummary` to sum historical details;
- updates the Asset.

Thus independent asset keys do not remove the potentially growing aggregate-query cost. An index narrows the history to an asset but does not necessarily make summing that history constant-time.

`AssetServices.reserve#AssetForOrderItem` (`:667`) also finds candidates and rereads selected assets, then creates Reservation and Detail records per selected asset (`:1053,1056`). Assembly components recursively call reservation (`:1079`). Count candidate/selected assets and component depth as workload dimensions, even with no waiting for locks.

**Change:** investigate a correctness-equivalent incremental balance path for normal durable transactions, with reconciliation as a separate job; verify delete/correction/retry semantics. Preserve the current transaction-cache and sim behavior, including the existing `no-tx-cache` attribute. Do not assume enabling a cache alone makes these paths equivalent or eliminates every query.

## 4. JS host-boundary and decimal costs are material

Financial paths are not only high-level orchestration:

- `OrderServices.get#OrderItemTotal` (`:192`) performs multiple host-decimal operations and rounding for every invocation.
- `InvoiceServices.get#InvoiceTotal` (`:245`) performs multiplication, addition and rounding for every item.
- `LedgerServices.calculate#AcctgTransTrialBalance` (`:461`) sums host decimals over entries.
- `OrderInfoServices.check#OrderPreApprove` (`:813`) has 20 XML loops, 33 script blocks, and many financial/collection checks.

The planned decimals remain Rust host objects. Migrating arithmetic to `.add()`, `.multiply()`, `.setScale()`, comparisons, etc. introduces repeated JS-to-Rust calls, host wrappers and temporary values. Bare variables resolve through the context proxy; entity properties and collection operations also cross the bridge. Actual cost depends on the unimplemented bindings/code generator.

**Change:** preserve direct host values, avoid whole-map/list copies, and reuse compiled functions within each worker runtime. Benchmark native bulk decimal folds/item-total helpers against generated JS using identical rounding and transaction-visible data. Use lexical locals for proven local temporaries only where the live context/migration contract allows it. Do not replace decimals with JS numbers or rewrite context access in ways that alter semantics.

**Engine decision:** keep QuickJS until a matched native-vs-JS profile shows excessive CPU/GC. Eliminating repeated work or coarsening host calls may outperform an engine swap. V8 can accelerate JS loops; it cannot eliminate SQL or make thousands of required host calls disappear.

## 5. Collection algorithms can bottleneck JS even with resident data

- `OrderBulkServices.xml:207,221`: each updated input item searches the existing item list; missing-item detection performs list membership checks. Worst-case matching work is quadratic in input/existing item counts.
- `TaxServices.xml:128`: for each order item, scan all items again to find child discounts. Tax result matching also searches the item list (`:149`). This is repeated computation, not business-resource contention.
- `OrderInfoServices.xml:362`: repeatedly filter the whole item list by part. `:378` and subsequent script code perform database reads and related-value accesses inside the part loop.
- `PriceServices.xml:82`: quantity filtering and maximal-match selection run over qualifying prices. `:157` loops over configured modifiers, copies context, reads related parameters/service registrations, and invokes dynamic services. `get#ProductPrice` does at least two price-type service calls before optional fallbacks.

**Change:** index existing items by external sequence, use Sets for membership, group items by part/parent once, pre-index discounts and tax-result matches, and prefetch repeated reference data. Preserve duplicate-key resolution, ordering, and transaction visibility. Benchmark rules/modifier count as well as line count.

## 6. Hidden queries and deployment-dependent work

Relationship expressions such as `orderHeader.parts`, `orderHeader.items`, `orderItem.product`, and `priceModify.parameters` may execute entity finds. Scripts contain database calls that the XML-loop census does not see. A cache hit is possible, not guaranteed. Runtime statement traces must include these accesses and ECA bodies.

Configured service hooks also leave the source graph:

- pricing modifiers: `PriceServices.xml:157`;
- order approval hooks: `OrderInfoServices.xml:900`;
- tax gateway: `TaxServices.xml:144`;
- promotion implementations in `PromotionServices.xml`;
- payment/carrier interfaces implemented by other components.

`PriceServices.run#ProductPriceKS` (`:34`, since removed with all Drools/KIE from the component) called KIE if configured. No KIE replacement was found in the inspected Rust architecture/implementation plans. Its missing-tool fallback is not evidence of equivalent production pricing.

`ShipmentServices.complete#ShipmentPack` (`:1934`) calls normalization and packing with `force-new` (`:1950,1956`) before optional downstream processing. `print#ShipmentLabels` downloads missing labels and calls the print service (`:1909,1927`). Google-party services perform outbound HTTP. These waits need isolation from writing workers, but acknowledging an accepted processing job is not equivalent to acknowledging completed business work.

**Change:** explicitly bind dynamic implementations and classify which stages gate business completion. Processing-node separation requires a concrete handoff and preserved error/transaction semantics; node labels alone do not remove synchronous work.

## 7. Worker capacity, write amplification, and background throughput

The architecture's default pool rule gives at most 41 request + service workers combined. Achieving 100k/s with 41 request workers requires average occupancy <=410 microseconds, and service workers reduce that request allowance. The audited multi-step paths make this a capacity-sizing issue even before any JS measurement. More accepted sockets do not unblock synchronous host database calls.

Every additional item/header/payment/ledger update consumes index/WAL/CPU and may produce audit/feed work. Commit-time email/notification SECAs and feeds need sustained consumer capacity. Excluding business contention does not exclude engine/storage/pool bottlenecks. Separating reports from the writer does not remove reads required for totals, validation, account resolution or correctness.

For B business transactions/s, use measured values, not single-row assumptions:

```text
SQL commands/s      = B × mean commands per completed business transaction
WAL bytes/s         = B × mean WAL bytes per completed business transaction
occupied workers    >= B × mean worker occupancy seconds
application cores   >= B × mean application CPU seconds per business transaction
```

Size reserve/parking budgets and deployment resources against these quantities. Audit nested independent commits separately; do not count them as additional completed business processes.

## Implementation priorities and proof

1. First fix or explicitly redesign order/invoice total amplification, then duplicate reads and historical asset summation. Preserve observable business semantics with parity tests.
2. Replace quadratic matching/grouping loops with indexed algorithms.
3. Build a real generated-JS + entity/transaction facade spike for: simple party creation; a bounded order import; invoice finalization with accounting; reservation with independent assets; payment application. Use 1/10/100-line variants where applicable.
4. Measure native orchestration versus generated QuickJS through the **same facades, callbacks, SQL and commit boundaries**. Separate JS CPU/bridge/GC, facade CPU, SQL wait, commit wait, and cleanup. A fake host-call benchmark measures only the bridge, not transaction throughput.
5. Trace all executed commands and resulting state. Run required ECAs, auth, audit and feed behavior. Record cold/warm caches, reference/history sizes, rules, selected assets, and component depth.
6. Benchmark full authenticated durable completion with bounded workers and sustained background draining. Only then choose whether QuickJS, native hot helpers, database changes, or deployment capacity are necessary.

**Bottom line:** for small independent transactions, QuickJS orchestration is plausible. For current multi-line order/invoice paths, faithfully compiling the existing XML to cached JS carries triangular recalculation and substantial host/database fan-out forward. Fix that workload shape before claiming that a faster script engine will deliver 100k/s.

## Execution alternatives: native speed versus scripting UX

### Recommendation

Do not replace the current engine with a new expression language merely on the strength of static counts. The lowest-risk route is existing XML/JS semantics, algorithm fixes, and coarse native helpers or native implementations of measured hot services. If native XML execution is worth pursuing, build a shared validated action IR and test an optional ahead-of-time Rust backend. Keep JS as the escape path for authored scripts and unsupported expressions. This is a proposed alternative, not an already accepted architecture change.

The previous suggestion of a small typed expression language understates migration work: Mantle already embeds closure-style collection operations, arbitrary scripts, dynamic context writes, mutable objects, exceptions, configured calls and Java integrations. A language sufficient for all of that is not small. Native structural XML actions are substantially easier than compiling every embedded expression/script to Rust.

| Alternative | Mantle fit and limits |
|---|---|
| Whole-service QuickJS + native bulk helpers | Lowest new compiler complexity; preserves the planned language/browser story. Best initial experiment. Fine-grained proxies, interpretation and GC remain outside the helpers. |
| Replace QuickJS with V8 | Keeps JS language, but bindings/runtime caches/interrupt behavior need replacement and testing. JIT may help large loops; it cannot eliminate host decimal calls, SQL or repeated business work. Larger runtime and per-worker costs need measurement. |
| Predecoded Rust action executor + JS expressions | Structural actions call Rust directly and definition edits need no rustc. Can lose to whole-service JS if every condition/set requires an engine entry and state synchronization. No demonstrated speed ordering. |
| XML/action IR to Rust AOT + JS fallback | Removes interpreter/bridge cost for supported regions and allows typed locals. Highest-performance potential for native regions, but fallback, context visibility, deadlines, reload and error mapping must be designed. Arbitrary JS is not automatically compiled. |
| Rust script bodies | Native execution without implementing another language, but authors see Rust typing/ownership and build latency. Not equivalent scripting UX and not a sandbox. |
| New DSL, Rhai or Lua | Introduces another migration and semantic surface; does not itself solve financial operators, dynamic context, or query costs. No audited evidence justifies this swap. |
| WebAssembly | Useful isolation/fuel options, but not a compiler for existing JS/Groovy and not inherently faster. Entity/database handles require imports and lifetime management. Cannot simply run existing live synchronous facade objects unchanged. |
| Custom Cranelift/LLVM JIT | Could compile a defined IR without rustc latency. Still requires a compiler frontend, type/alias semantics, debugging, cancellation and safe code lifetime management. Too much new infrastructure before a measured need. |

A shared IR is useful only if it represents the same business semantics, not two independently maintained interpreters. Start with complete functions or substantial regions. A few generated arithmetic instructions separated by repeated JS fallback calls can erase native gains.

### Edge cases that determine whether AOT is actually safe

These are observed source constructs and framework contracts, not a claim that static inspection found every possible edge case.

1. **Missing/null/false/zero/empty semantics.** Existing Groovy treats unset context names as null, empty collections as false, and decimals according to Groovy numeric rules. The planned JS migration explicitly changes/coerces these cases. The backend must implement the chosen contract for optional values, Elvis, membership, equality and short-circuiting; Rust truthiness does not provide it automatically. Do not mix legacy Groovy semantics and migrated JS semantics between backends.
2. **Financial precision.** Preserve per-line rounding before summation, scales, currency rounding, divide-by-zero and overflow errors. JS numbers are not an alternative to decimals. `rust_decimal` has narrower capacity than arbitrary Java BigDecimal; parity needs boundary vectors, not only typical values.
3. **Dynamic context and aliases.** `TimeServices.store#TimeEntry` writes keys through `context.put(fieldName, fieldValue)`; pricing copies context then merges dynamic service results; callers use `out-map="context"`. The census found 93 `out-map-add-to-existing="false"` attributes across tracked service/entity XML. Merge versus replace, shadowing and missing versus explicit null must remain observable. Native locals cannot become stale after script/service writes. Use explicit escape/synchronization rules; optimizing globals as immutable locals is unsafe.
4. **Mutable entity/list identity.** Scripts modify entity fields, refresh/clone values, mutate collections, and follow relationships. Multiple variables can alias the same value. Copies, native borrows or serialized snapshots must not silently change identity, dirty-field tracking or later reads. Avoid holding conflicting Rust borrows across reentrant callbacks.
5. **Implicit queries and transaction overlays.** Relationship getters can query; cached values and views can behave differently under the transaction cache/sim overlay. There are 19 `no-tx-cache="true"` attributes in the scanned XML. Compilers must not hoist reads, memoize properties or push folds into SQL without proving transaction visibility and callback equivalence.
6. **Dynamic service implementations.** Tax, pricing, promotions, approval, payment and carrier integrations resolve names at runtime. Even a literal service name can be overridden by a component. A native call must use the service facade/registry or a version-guarded specialization that preserves validation, auth, ECAs and transaction attributes. Direct calls to a frozen generated callee are not generally equivalent.
7. **Transactions and partial success.** Nested defaults join; `force-new` commits independently; shipment packing deliberately preserves committed progress after later failures. Error messages, rollback-only state, exceptions and `ignore-error` are different mechanisms. Preserve savepoint requirements and pool parking/reserve rules. Native execution cannot collapse these into one all-or-nothing function.
8. **ECA ordering and reentrancy.** Before/after entity hooks, original-value snapshots, `set-results`, post-service callbacks and commit callbacks have distinct timing. Order-major-change uses shared context to prevent reentrant recursion. Preserve immediate totals observed by later actions. Coalescing hooks or grouping writes is a semantic change unless explicitly defined and tested.
9. **Control flow and cleanup.** XML has early returns, breaks/continues, loops and recursion; script bodies have try/catch/finally even though the census found no XML try/catch/finally elements. `return error="true"` adds framework error state, not necessarily an ordinary thrown exception. Ensure cleanup on every exit, including iterators, context frames and transaction checkout. Panic handling is not a substitute for business errors.
10. **Deadlines and isolation.** QuickJS currently plans interrupts and a script memory cap. Native loops need deadline/budget checks; a timeout cannot safely kill a Rust thread. Blocking database work still needs driver cancellation. Compiled author-supplied Rust is privileged native code, not a sandbox, and arbitrary native allocations lack the JS heap cap. Restrict the backend to trusted component definitions or provide separate isolation.
11. **Async/thread-local values.** `DuplicateServices.check#AllPartyDuplicates` submits futures and joins batches of ten. Workers need independent contexts/transactions, bounded queues and reserved-worker rules. QuickJS values cannot cross worker runtimes; serialize approved values/handles rather than capturing JS closures or borrowed entities into jobs.
12. **Resource strings, browser code and runtime definitions.** Templates, database-configured hooks, dynamic views, localization and live expression expansion remain runtime facilities. Rust AOT does not replace browser JS. Keep the restricted data-template path separate from trusted executable expressions. Newly loaded/changed definitions need a compatible fallback or explicit rebuild-required error, not stale native execution.
13. **Java integrations and external scripts.** `PartyServices.find#Party` points to an external Groovy file; KIE pricing and Java file/date/blob/concurrency classes appear in source. Neither rustc nor V8 makes these APIs exist. Inventory/migrate external script files and configured components too. Missing integrations must not silently become no-op "fast" implementations.
14. **Code lifetime and reload.** Pin a definition/code generation for an in-flight transaction and its callbacks. On edits, invalidate affected services, hooks and generated artifacts. Native dynamic loading needs ABI and safe code-lifetime machinery; process replacement is simpler. On an existing database, a new process must not also try to start a conflicting embedded database. A scripting-style watch command can automate build/restart, but rustc still incurs compile/link latency.

### Practical native-backend design, if the spike justifies it

```text
XML + authored JS expressions
  -> resolved definitions + validated action IR
  -> current JS backend / optional native Rust backend
  -> same service/entity/transaction facades
```

Lower a documented, provably supported subset. Initially reject unsupported native regions explicitly or retain them wholly in JS; do not pretend arbitrary JS compilation is implemented. Cache native artifact identity using source/dependency hashes, backend version, framework ABI and build target. Native artifact code and its metadata must agree. Prefer production AOT and a development watch/build/process-replacement command over compiling during readiness or unsafe hot unloading.

Keep one scripting language and the existing generated TypeScript declarations. Native hot helpers should operate on runtime host values directly, not copied JSON. If each JS fallback requires serializing all context, this architecture is unlikely to meet its purpose.

### Decision experiment

Compare: (A) current whole-service JS, (B) JS with native coarse helpers, and (C) native service/action regions. Fix workload amplification equally for all three. Use identical business semantics and SQL, and include callbacks plus durable completion. Measure application CPU, host crossings, allocations/GC, SQL/commit wait, worker occupancy, compilation/startup cost and tails. Include cold/warm caches, error exits, transaction cache/sim, dynamic service overrides and context aliasing. Benchmark 1/10/100-line order/invoice cases, asset history growth, and payment application.

At 100k completed transactions/s, an extra 100 microseconds of application CPU consumes ten fully occupied cores, but eliminating it increases end-to-end throughput only if application CPU/worker occupancy is limiting. No backend choice by itself removes the audit's repeated SQL, WAL or durability costs.

**Decision:** start with algorithm fixes plus native coarse helpers. Keep optional IR-to-Rust AOT as the next measured experiment, not a promise of full Mantle compilation. Do not introduce a new DSL or JIT until this comparison establishes why the existing language/runtime cannot satisfy the resource budget.
