# Test harness: `moqui test`, a Spock replacement for JavaScript

> Archived 2026-10-16: built. Kept as the design record; the spec format, script API and runner sections below are the only description of `moqui test` outside the code and `runtime/component/mr-mantle-usl/test/README.md` until a `docs/Testing.md` is written from them (listed in [Remaining-Work](../Remaining-Work.md)). The build-order paths below are as they were.

Status 2026-10-15: all steps built. The eight mantle-usl specs are ported and pass (84 features) and the Rust flow tests are deleted; see [MantleFlowTests](MantleFlowTests.md). First user: mantle-usl (replaces [MantleFlowTests](MantleFlowTests.md) when the port passes). Porting guide: `runtime/component/mr-mantle-usl/test/README.md`.

## Built (steps 1–4)

| Piece | Where | Checked by |
|---|---|---|
| F1 effective time: `ec.user.setEffectiveTime`, per context; `nowTimestamp`, date filters with no date (script and view `FindValues::now`), entity-auto and data-load `fromDate` defaults, audit `changedDate`, dbresource `versionDate`. Login, authz, timing and jobs keep the clock | `context/factory.rs`, `user/login.rs` (`now_effective_*`), `service/call.rs` | `framework_gaps_tests::effective_time_*` |
| Entity-auto `fromDate` default (Java `checkFromDate`), missing before | `service/call.rs` `run_entity` | same |
| F2 `tempSetSequencedIdPrimary`/`tempResetSequencedIdPrimary` | `entity/sequence.rs`, `entity/facade.rs`, JS `ec.entity` | `sequence::tests::temp_set_*`, `framework_gaps_tests::temp_sequenced_*` |
| F3 data loader check mode: `check()`/`check(messages)`, all formats, nested records with parent keys, Java's messages | `entity/load_feed/check.rs` | `framework_gaps_tests::data_loader_check_*` |
| F4 `ec.artifactExecution.disableAuthz/enableAuthz` | `context/factory.rs` `script_*_authz`, `tools_host.rs` | `framework_gaps_tests::script_disable_*` |
| F5 `ec.factory.waitWorkerPoolEmpty` | `context/factory.rs` | `framework_gaps_tests::wait_worker_*` |
| F6 ScreenTest: method, `baseScreenPath`, errors collected, counters; JS `ec.screen.makeTest()` | `screen/screen_test.rs` `test_render`, `ec_screen.rs` | `screen::tests::print::a_screen_test_render_*`, fixture `Basics` REST feature |
| F7 JS `Date` to time stamp | `script/runtime/convert.rs` | effective time test |
| F8 `parallel` | `testkit/run.rs` | `testkit::tests::parallel_*`, fixture `Basics` threads feature |
| Rewrite (power asserts, Groovy `==` and truth) | `testkit/rewrite.rs` | `testkit::rewrite::tests` |
| Prelude and driver | `testkit/prelude.js`, `testkit/run.rs` | `testkit::tests` (21) |
| `moqui test` command: fixture (fresh, snapshot, existing), discovery, `suite.js`, `dependsOn`, console report, JUnit | `moqui-host/src/test_cmd/` | `test_cmd` unit tests, `crates/moqui/tests/cli/test.rs` on `crates/moqui-host/tests/fixtures/test-component` |

Changes from the design below:
- `then`/`expect` conditions are only the expression statements directly in the block; `assert(condition, message)` is added for loops and callbacks (Groovy's `assert`).
- Spec-level `setup`/`cleanup` get no `ctx`; feature blocks do. A `then` the rewrite could not see is a registration error.
- A `parallel` function returns JSON-friendly values; `opts` are `transaction` (default true) and `timeout` (default 120s).
- `suite.js` is `suite(["SpecA", "SpecB"])`, read without running it.
- The per-feature "last N log lines" in a failure is not built: the full log is `runtime/log/moqui.log` (the runtime's, also for a scratch runtime), and the summary prints its path.
- Not built: `moqui lint` for spec files, TypeScript declarations for the spec globals.
- Authz guards a spec leaves are dropped after the spec, not after each feature, since a `setupSpec` may disable authz for the whole spec.
- Time stamps reach scripts as epoch millis text (existing behaviour), which matches the Spock data checks that write `${effectiveTime}`.


## Context

mantle-usl's services are hand-ported to JavaScript in the fork `moqui/mr-mantle-usl`. The Spock specs that check them (`runtime/component/mantle-usl/src/test/groovy`, 10 files, ~4,600 lines) were ported to Rust as `crates/moqui-host/tests/mantle_flows.rs` (2,785 lines) plus `mantle_support/mod.rs`. That put the component's tests in the framework repo, required a Rust toolchain, and needed a conversion layer for every result. It also dropped strictness, because mr-moqui has no `tempSetSequencedIdPrimary` or `setEffectiveTime`: ids are looked up, dates are skipped, and nested `<items>` elements are skipped in checks.

The goal is to build a `moqui test` command in the binary that runs JavaScript spec files from a component's `test/` directory, in the same QuickJS engine as the services, with Spock's conveniences:
- `setupSpec`/`setup`/`cleanup`/`cleanupSpec`;
- ordered features and shared state;
- `given`/`when`/`then`/`expect`/`cleanup` blocks with power-assert failure output;
- `where:` data tables;
- a suite order;
- data checks, pinned ids, a pinned clock, screen/REST tests and multi-thread tests.

Then port the mantle-usl specs **from the Groovy originals** and retire the Rust flow port.

Decided:
- **Form.** Features and specs are objects whose fields are block functions (`given`, `when`, `then`, `expect`, `cleanup`, `where`), a familiar pattern from Vue component objects and similar frameworks. This replaces Spock's `when:` labels, because JS rejects a label before `const`/`let` and editors would flag it.
- **Assertions.** Plain expression statements in `then`/`expect` are rewritten into power asserts with Groovy `==` and truth, not an explicit `expect()` API.

## What exists and is reused

| Need | Existing piece |
|---|---|
| Scratch runtime + embedded Postgres + `load types=all` + boot | `crates/moqui-host/tests/mantle_support/mod.rs:80-130` (`start`), `moqui_host::load_data`, `db/embedded.rs` `EmbeddedCluster::start`, `save_db`/`load_save` (`:130,143,202-216`), `load_with_factory` pattern `crates/moqui-host/src/lib.rs:597-644` |
| CLI dispatch | `crates/moqui-host/src/lib.rs:200-278` (`USAGE`, slice match); model on `dispatch_load` `:302` |
| Run a JS file against the entered context | `moqui_core::script::run_script_location` (`script/runtime/facades.rs:174`), `with_context`/depth-0 engine (`script/runtime.rs:645`) |
| JS parser for the rewrite | `oxc_parser`/`oxc_ast` 0.139, already in moqui-core (`script/parse.rs`) |
| Service sync call returning a plain object | `script/runtime/services.rs:86-276` (returns `null` + `ec.message` error on failure) |
| Data loader record shapes (real PKs, nested relations) | `entity/load_feed/loader.rs:39-56,130` (`Record`, `Shape`, `parse_top`); JS `makeDataLoader` `script/runtime/tools_host.rs:219-240` |
| Sequence banks | `entity/sequence.rs:93-197` |
| Authz guard | `context/factory.rs:638`, `context/artifact.rs:170-200` |
| ScreenTest (Rust, GET only) and full socketless HTTP | `screen/screen_test.rs`, `screen::serve_at` (`screen/mod.rs:87`) with `WebRequest` (`web/mod.rs:46`) |
| Worker pool | `service/pool.rs:330,375,394` |
| Behaviour oracle for checks | Rust `mantle_support::check` (to be replaced), Java `EntityDataLoaderImpl` check mode |

## Spec file format

Files: `<component>/test/*.spec.js`. Optional `<component>/test/suite.js` gives the order, porting `MantleUslSuite`. The spec name defaults to the file name.

Each spec and each feature is an object whose fields are options and block functions, as in Vue component objects. Every file is plain, valid JavaScript.

**Data between blocks.**
- Each block receives `ctx`, which holds the `where` row's fields plus every field the earlier blocks returned. `given` returns into `when`, and `when` returns into `then`.
- The returned object stands in for Spock's method-local variables that `then:` reads.
- State shared across features (Spock `@Shared`) is a file-level `let`.

```js
// mantle-usl/test/OrderToCashBasicFlow.spec.js
const effectiveTime = Date.now()
let cartOrderId, cartOrderPartSeqId, setInfoOut          // @Shared fields become file-level lets

spec({
    dependsOn: ["AssetReservationMultipleThreads", "OrderProcureToPayBasicFlow"],
    tempSequences: { start: 55500, banks: { "mantle.order.OrderHeader": 10, "mantle.ledger.transaction.AcctgTrans": 50 /* ... */ } },
    setupSpec() { ec.user.setEffectiveTime(effectiveTime) },
    setup() { ec.artifactExecution.disableAuthz() },
    cleanup() { ec.artifactExecution.enableAuthz() },
    // cleanupSpec() { ... }   temp sequences and effective time are reset by the runner after it
})

feature("create Sales Order", {
    when() {
        login("joe@public.com")
        const priceMap = call("mantle.product.PriceServices.get#ProductPrice", { productId: 'DEMO_1_1' /* ... */ })
        const addOut1 = call("mantle.order.OrderServices.add#OrderProductQuantity", { /* ... */ })
        cartOrderId = addOut1.orderId
        const dataCheckErrors = checkData(`<entity-facade-xml>
            <mantle.order.OrderHeader orderId="${cartOrderId}" entryDate="${effectiveTime}" grandTotal="200.68"/>
        </entity-facade-xml>`)
        return { priceMap, dataCheckErrors }
    },
    then({ priceMap, dataCheckErrors }) {
        priceMap.price == 16.99            // HostDecimal compared by value, as Groovy ==
        priceMap.priceUomId == 'USD'
        dataCheckErrors.length == 0
    },
})

feature("call REST API (#requestMethod, #screenPath)", {
    where: [
        { requestMethod: "put", screenPath: "s1/mantle/orders/productQuantity", parameters: { /* ... */ }, containsTextList: ['55800'] },
    ],
    expect({ requestMethod, screenPath, parameters, containsTextList }) {
        const str = restTest.render(screenPath, parameters, requestMethod)
        !str.errorMessages                  // Groovy truth: empty list is false
        containsTextList.every(t => str.assertContains(t))
    },
})
```

### Script API (globals in the spec engine only)

- **`spec({...})`**, once per file:
  - `dependsOn`: specs that run first when this spec is selected alone;
  - `stepwise`: skip the remaining features after a failure (Spock `@Stepwise`). Default false, as Spock;
  - `allowMessageErrors`, `timeout`;
  - `tempSequences: {start, banks}`, which is applied before `setupSpec` and reset after `cleanupSpec`;
  - the methods `setupSpec`, `cleanupSpec`, `setup`, `cleanup`, with Spock semantics. Cleanups always run.
- **`feature(name, {...})`**, in declaration order:
  - blocks:
    - `given(ctx)`, then `when(ctx)` paired with `then(ctx)`, or `expect(ctx)` alone. Each may return an object that is merged into `ctx`;
    - `cleanup(ctx)`, which always runs;
  - `where`: an array of row objects, or a function returning one. Each row is its own reported case (`@Unroll`), and `#field` in the name is filled from the row;
  - `ignore: "reason"`, `timeout`, `allowMessageErrors`.
  - A feature with `then` but no `when`, or a misspelled block name, is a registration error.
- **`call(name, params)`**: sugar for `ec.service.sync().name().parameters().call()`. It **throws** at the call site with the service name and the `ec.message` errors. Raw `ec.service.sync()` stays available.
- **`checkData(xml)`**: wraps `ec.entity.makeDataLoader().xmlText(xml).check()`, logs each error and returns the error array.
- **`login(user, password = "moqui")`**: throws if `loginUser` returns false. **`logout()`**.
- **`parallel(n, fn, args, {transaction: true, timeout})`**: runs `fn(threadNum, args)` on n threads, each with its own context and engine. `fn` is re-evaluated from its source, so it must not close over variables; `args` are JSON-cloned. It returns the results, and a thread error fails the feature.
- **`screenTest(baseScreenPath)`**: sugar for `ec.screen.makeTest().baseScreenPath(...)`.
- **`uploadedFile(fileName, text, contentType = "text/plain")`**: an uploaded file holding the text, the `FileItem` that services such as `save#InvoiceContentFile` and `upload#OfxVersion1` take.
- Helpers: `sum(list, field)` and `dec(x)` for decimal sums and conversion; `log` is `ec.logger`.

### Power asserts (source rewrite, `crates/moqui-core/src/testkit/rewrite.rs`)

1. **Finding conditions.** The spec source is parsed with oxc, and nothing is rewritten before parsing.
   - For each `feature(...)` call whose second argument is an object literal, it finds the `then` and `expect` members, whether written as methods, arrow functions or function expressions.
   - Every top-level expression statement in their bodies is a condition, as in Spock's `then:` block. Assignments are skipped, as Spock does. A call that returns `undefined` passes (Spock's void-method rule).
   - Generic linters may report `no-unused-expressions` in these bodies; the porting guide documents that.
2. **Span edits.** Each condition is rewritten with span-based text insertions, with no codegen and no new lines, so error line numbers match the file:
   - comparison operators become `__cmp(op, a, b)`, with Groovy semantics: decimals, numbers and numeric strings compare by value, `null == undefined`, and arrays and maps compare deeply;
   - `!x` and bare values use Groovy truth (empty array, map or string, and zero, are false);
   - identifiers, member reads and calls are wrapped in `__v(idx, expr)` to record values for the diagram.
3. **Unrewritten blocks.** Each rewritten function is stamped. A `then` or `expect` the rewrite could not see, such as a feature object built in a helper function or a loop, is a registration error naming the feature, rather than a block of conditions that never fail. Use `where` rows instead.
4. **Failure output.** A failure prints the Spock-style diagram, with values aligned under their columns, plus `file:line`. Values are formatted as follows: `HostDecimal.toString()`, entities as maps, long values clipped.

## Runner: `moqui test`

```
moqui test [component ...] [--spec Name[,Name]] [--feature substring] [--db fresh|snapshot|existing]
           [--junit <dir>] [--keep] [--log-level ...]
```

Code: `crates/moqui-host/src/test_cmd/` (`mod.rs` dispatch, `fixture.rs`, `discover.rs`, `run.rs`, `report.rs`) plus `crates/moqui-core/src/testkit/` (`rewrite.rs`, `api.rs` with the host functions, `prelude.js` via `include_str!`).

1. **Resolve.** Use `runtime_framework_dirs()`. The components are the arguments, or every component that has a `test/` directory, in component dependency order (so SimpleScreens runs after mantle-usl later).
2. **Database** (`fixture.rs`, extracted from `mantle_support::start` into a public `moqui_host::test_fixture`):
   - `fresh` (default): a scratch runtime under `/tmp/mt-<pid>` (a short socket path), with `conf` copied and `base-component` and all runtime components symlinked. It runs `load_data(types=all)` in process, starts `EmbeddedCluster`, and removes the scratch runtime at exit unless `--keep`.
   - `snapshot`: the same, but the loaded DB is saved to `runtime/save/test-<hash>/moqui.dump`. The hash covers the entity and data files and the binary version. Later runs restore it with `load_save`.
   - `existing`: use `runtime/db/embedded` as loaded (the `gradle test` on your DB workflow). This mutates data, and the runner warns.
3. **Boot.** Follow the `load_with_factory` pattern: `logging::install`, with full logs to `<runtime>/log/test.log` and only progress and failures on the console. Then the factory, the entity facade and search attach.
4. **Discover.**
   - Order: `suite.js`'s list first, then the remaining `*.spec.js` alphabetically.
   - `--spec X` expands to the `dependsOn` closure, kept in suite order (this replaces the Rust `ensure_*` prerequisite flags).
5. **Run each spec** on the main thread:
   - Make a new context, `enter()` it and clear the script stack.
   - Rewrite the source and evaluate it once at depth 0 with the **engine time limit off** (or the spec or feature `timeout`). The prelude's `spec`/`feature` calls only register.
   - Rust then drives `setupSpec`, then for each feature (each `where` row) `setup`, the body and `cleanup`, then `cleanupSpec`, calling the registered functions by index, so Rust owns timing, timeouts and log capture.
   - **After each feature:**
     - if `ec.message.hasError()` and errors are not allowed, the feature fails with those errors;
     - then clear the messages and drop any authz guards left open.
   - **After the spec:**
     - reset its temp sequences and the effective time;
     - log out;
     - `waitWorkerPoolEmpty(50)`;
     - exit the context.
6. **Report** (`report.rs`):
   - One console line per feature: `OrderToCashBasicFlow > create Sales Order  PASSED 1.24s`.
   - Failure details: the power-assert diagram or the thrown error with its spec `file:line`, the `ec.message` errors, data-check errors and the last N log lines of that feature.
   - A summary.
   - With `--junit <dir>`, `TEST-<component>.<Spec>.xml` (testsuite, testcase, failure, system-out).
   - The exit code is 1 when any feature fails.

## Framework additions (Java-parity APIs; also usable by services)

- **F1 `ec.user.setEffectiveTime(ts)`**:
  - accepts millis or timestamp text, with null to clear;
  - stored per ExecutionContext (Java: `UserFacadeImpl.effectiveTime`, `:62,469,637`);
  - `ec.user.nowTimestamp` and the Rust `now_value`/`now_millis` (`user/login.rs:366-380`, `script/runtime/helpers.rs:236`) honour it;
  - audit the ~40 direct `now()` calls in moqui-core and route only the ones whose Java counterpart uses `getNowTimestamp` (18 Java files). Audit stamps and job scheduling stay on the wall clock.
- **F2 `ec.entity.tempSetSequencedIdPrimary(name, next, bankSize)` / `tempResetSequencedIdPrimary(name)`**: overwrite and clear the in-memory bank in `SequenceBanks`, factory-wide as in Java (`EntityFacadeImpl.groovy:1908-1916`).
- **F3 Data loader check mode** in `entity/load_feed`:
  - `check()` returns error strings, with the Java message wording;
  - it uses the real PKs from `Shape`, and nested relationship elements inherit their parent keys;
  - comparison is type-aware: numbers by value, timestamps from millis or text compared as instants, and empty equals null;
  - only attributes present are compared;
  - JS binding: `makeDataLoader().check()`.
- **F4 `ec.artifactExecution.disableAuthz()`/`enableAuthz()`**: script-side bindings that keep a stack of `AuthzGuard`s on the context; any guards left open are dropped when the context exits.
- **F5 `ec.factory.waitWorkerPoolEmpty(retryLimit)`**: polls the pool sizes every 100 ms.
- **F6 ScreenTest** reworked over `screen::serve_at`, adding:
  - the method, `baseScreenPath`, and a test session with Java ScreenTestImpl's session token behaviour (to confirm against `ScreenTestImpl`; RestApiTests sends `moquiSessionToken: 'TestSessionToken'`);
  - a JSON body;
  - render results: `output`, `errorMessages`, `status`, `renderTime` and `assertContains`;
  - counters: `renderCount`, `errorCount`, `renderTotalChars` and `startTime`;
  - JS binding `ec.screen.makeTest()`.
- **F7 JS `Date` to host Timestamp** in `script/runtime/convert.rs`. Today a `Date` becomes an empty map, and the specs pass `new Date(effectiveTime + ...)`.
- **F8 `parallel` host support**:
  - spawn threads, each with `factory.new_context()`, entered;
  - evaluate the function source in the thread's own engine;
  - optionally run in a transaction (`TransactionManager` begin/commit);
  - join with a timeout, and return the results or errors.

## Build order and delegation

1. **Write `plans/TestHarness.md`** (this design) and a porting guide, `mantle-usl/test/README.md`. The guide's Groovy-to-JS table:

   | Groovy | JS |
   |---|---|
   | `ec.service.sync().name(n).parameters([a:1]).call()` | `call(n, {a: 1})` |
   | `"""...${x}..."""` | template literal |
   | `.size()` | `.length` |
   | `@Shared` | file-level `let` |
   | `when:` locals read in `then:` | returned from `when()`, destructured in `then(ctx)` |
   | `new Timestamp(t)` | `t` |
   | `list.sum { it.f }` | `sum(list, 'f')` |
   | `where:` table | `where: [...]` field |
   | `Thread.start` + `CountDownLatch` | `parallel()` |
   | `logger` | `log` |
   | BigDecimal arithmetic | decimal methods |

2. **Framework pieces.** F1, F2, F4, F5 and F7 are small and independent and can run as parallel agents. F3 and F6 are medium and each gets its own agent. F8 comes after F4.
3. **The rewrite and the runner.** `testkit/rewrite.rs` with unit tests (diagram snapshots, Groovy `==` and truth, decimals, unchanged line numbers, method/arrow/function forms, unrewritten-block detection). Then `test_cmd` with the fixture extracted from `mantle_support`.
4. **The harness's own tests.** A fixture component `crates/moqui-host/tests/fixtures/test-component/test/` with passing, failing, `where`, stepwise, `dependsOn`, `parallel`, message-error and ignore specs. CLI tests in `crates/moqui/tests/cli` run `moqui test` on it and check the console output, the JUnit XML and exit codes. This uses SQLite or embedded Postgres, whichever the CLI tests already use.
**Pause.** Stop after step 4, before the port, for a manual review of the harness. The user may switch models here. Report:
- what was built;
- the results of the harness's own tests;
- one example spec with its failure output;
- any API changes from this plan.

Wait for the go-ahead before step 5.

5. **Port mantle-usl from Groovy**, one agent per spec, in suite order:
   - AccountingActivities, AssetReservationMultipleThreads, OrderProcureToPayBasicFlow, OrderToCashBasicFlow, OrderToCashTime, ReturnToResponseBasicFlow, WorkPlanToCashBasicFlow, RestApiTests, plus `suite.js`.
   - OrderTenantAccess is not ported, since it is out of the Spock suite and multi-tenant is deprecated.
   - Ports keep the Groovy checks verbatim, with pinned ids, `effectiveTime` dates and nested elements.
   - The Rust port is used only as a hint about which mantle services needed fixes. Service bugs found go into mantle-usl's JS.
6. **Retire the Rust flows.** Once `moqui test mantle-usl` passes at least the 8 flows the Rust port passes today:
   - delete `crates/moqui-host/tests/mantle_flows.rs` and `mantle_support/`;
   - replace `plans/MantleFlowTests.md` with the harness doc and a status table;
   - update `plans/Remaining-Work.md` (the ScreenTest harness item closes).
   - `mantle_search.rs` and other Rust tests that need the Mantle fixture switch to `moqui_host::test_fixture`.

## Verification

- `cargo test -p moqui-core testkit` (rewrite and check-mode units) and `cargo test -p moqui --test cli` (the `moqui test` fixture component: pass and fail output, JUnit, exit codes).
- Framework parity tests:
  - effective time shows in `ec.user.nowTimestamp` and in records a service stamps;
  - temp sequences give 55500, 55501, … and reset;
  - check mode against a loaded record with nested items;
  - `ec.screen.makeTest().render("s1/...", params, "put")`.
- `moqui test mantle-usl` runs the whole suite. `moqui test mantle-usl --spec OrderToCashBasicFlow` runs its `dependsOn` closure first. `--db snapshot` twice: the second run restores without loading.
- Break an assertion on purpose (e.g. `grandTotal`) and confirm the power-assert diagram and `file:line`. Confirm the JUnit XML parses.
- During the transition, run `cargo test -p moqui-host --test mantle_flows` alongside it, then delete it in step 6.

## Known risks

- Expression statements in `then`/`expect` are valid JS, but generic linters may flag them as unused expressions. `moqui lint` should learn `*.spec.js` and check that each `then` or `expect` is in a form the rewrite can see.
- The deadlock in AssetReservationMultipleThreads is a framework concurrency bug that `parallel` will surface. It is tracked separately and the spec may be marked `ignore` until fixed.
- F1's scope: routing too many `now()` calls through the effective time would break audit and job behaviour. Route only where Java does.
- Strict Groovy checks (pinned ids and dates) may expose legitimate ordering differences in mr-moqui. Adjust those checks with a comment rather than loosening the checker.
