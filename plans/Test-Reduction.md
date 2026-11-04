# Test reduction

A proposal in two parts. First, split the tests into two suites with different purposes: a small **regression suite**
that developers and agents run locally before every commit, and a separate **acceptance suite** of complete scenario
and system checks, run before a deploy or for general system validation, never as part of normal development. Second,
take the multiplication out of the regression suite: the walk-everything renders, the authorization doubling, the
access walks and the browser walks beyond one per app. Scenario checks move to the acceptance suite, duplicates are
deleted, and the rule tests stay. The regression suite's feature time drops by about 70% (1,729 s to about 510 s), and
walk-everything and every-mode render tests are gone from it for good. Nothing is moved or deleted yet: this document
was for review; it is now implemented ([Implemented](#implemented), Nov 4, 2026).

**The 10 minutes is a goal for `test-all` without the Rust build time, not a constraint.** The cut alone takes the
tests of `test-all` from about 37 minutes to about 13½; parallel spec workers ([Cheaper machinery](#cheaper-machinery))
take them to about 7¼, with every rule test kept. Application work runs only the app tests
([below](#framework-tests-and-app-tests)): about 9¼ minutes serial, 3½ with workers. On this machine a cold release
build takes about 5 minutes and the debug build for `cargo test` about 8, so with the builds no test run could fit in
10 minutes. Builds are measured apart: a test run should need a rebuild only after a change to the Rust code or to what
it embeds, and more test runs over time should reuse the binaries already built (`--nobuild`, sccache) rather than
rebuild.

## Implemented

Steps 0 to 7 are in, one commit per step in each repository. The measured `./moq test-all --nobuild` (Nov 4, 2026, this
machine): **7 min 39 s in all**, the 30 s incremental cargo build included, against about 37 minutes of tests before.

| Stage | Wall | Features |
|---|--:|--:|
| `moqui test` (framework, 4 workers) | 30 s | 342 |
| `moqui test demo` | 17 s | 10 |
| `test-client webroot --offline` | 50 s | 376 |
| `test-client dev-aide --offline` | 5 s | 44 |
| `test-client --serve` (framework, 3 servers) | 46 s | 147 |
| `moqui test` (apps, 4 workers) | 158 s | 2,777 |
| `test-client --serve` (apps, 3 servers) | 81 s | 13 |
| cargo build and test, `moq-test.sh` | about 72 s | |

Feature time by component (the serial sum, against its `test/budget`): mr-mantle 270 s of 300, mr-boss 201 of 220 and
its walks 32 of 36, webroot 30 of 34, dev-aide 27 of 30, mr-pass 13 of 15, framework 10 of 12, the others under 10.
`./moq test-apps` is the two app stages, about 4 minutes.

Where the implementation differs from the text below:

- **Budget file.** `test/budget` has a `server` and a `client` line, since `moqui test` and `moqui test-client` each see
  only their part. The budgets were set from the measured test-all at about 10% over it; the run reports each
  component's feature time against its budget.
- **Acceptance runs resolve `dependsOn`.** With no `--spec`, `--acceptance` takes what its specs' `dependsOn` names from
  the whole pool (mr-marble's sweeps bring Mantle's flows). `--spec component/Name` names one component's spec, which
  the workers use, as mr-boss and mr-mantle share spec names.
- **Kept, not as the tables say:** InvoiceOrderApply and InvoicePostOrder (E): their rules (apply, unapply, item
  details, bill-through items, the F429 posting order) are not in OrderToCashBasicFlow. SecurityCors stays its own
  spec: its setupSpec needs a conf with no foreign origin. CallClientFixtures stays in `test/`: CryptoClientFixtures
  depends on it. ProductsMethodFixtures and ProductsCostFixtures stay in `test/`: ProductsMethods and ProductsCosts
  depend on them. ArtifactIndex already built its index once; its 9 s is the build under test. The two Forum
  duplicate renders render other users over other data, so no check could move.
- **Not merged:** the sweeps moved to `acceptance/` as they were (the four Products sweeps, the four mr-marble ones);
  acceptance has no budget, so merging them is left to whoever next changes them.
- **The call walks** passed in acceptance in 3 to 7 s each; their 61 s timeouts did not reproduce.
- **Contacts** is split by rule: its duplicate pairs are ContactsDuplicates.
- **Found by the workers:** basic-comms `apply#DndHours` saved each do-not-disturb user's zone on the signed-in
  caller's account (fixed); mantle AccountingPortals left the token-lookup tarpit full for ten minutes (it now cleans
  up); ArtifactIndex reads files other specs write (it catches up before its unchanged-file check).
- **Per-feature cost.** The harness costs under 10 ms a case (260 of 3,163 app cases); the time is the specs' own
  service and data work, half of it in cases of 0.1 to 0.5 s.
- **Left for later:** duplicate renders the static scan missed (ProductsPriceLists renders PriceListDetail and three
  other screens in every mode; PaymentDetail/Refunds in qvt by AccountingPaymentRefunds and AccountingPayments; Review
  rendered in three modes without its own templates; OrderDetail tui by Payables and Collections), the column
  chooser's html and tui rule (a framework test now, since ColumnChooser's qvt check joined TimeEntries), and
  `create#InvoiceCreditMemo`, which artifact authorization refuses to finance.ar and finance.admin (AccountingAccess
  makes its fixtures with authorization off).

## Why

The last test-all ran its tests for about 37 minutes, besides the builds: 4,018 server features in 881 s (0.22 s each)
and 425 browser features in 814 s (about 2 s each). A page renders in milliseconds; the time is the multiplication.
The suite grew with screens × users × render modes × authorization on and off × apps, not with the rules it checks, so
every new screen, group or app made every run longer, and with all the apps built it heads for hours. The rules made it
so: "every new screen in AppStructure and TuiRender", "a walk for each scenario", phase Done rules needing access-spec
features and phone-width walks per phase, and agents followed them faithfully (the Manufacturing build alone added about
20 specs, 16 walks and an access-spec section per phase). An oversized regression suite is a performance bug of the
system as a whole, like reading the whole conf tree to get one value, and the `slow` flag only hid the worst of it.

Three quarters of the time is that multiplication: the walk-everything renders (350 s), the access specs with their
doubling and walks (162 s) and the browser walks (783 s), of 1,729 s. The rest, 434 s, is rule tests, the thing a
regression suite is for: they stay, and their time is the machinery's to cut.

## Two suites

| | Regression suite | Acceptance suite |
|---|---|---|
| Purpose | Catch a regression while developing: does what I changed break a rule? | Validate the system: complete scenarios, access and isolation walks, one screen sweep per app and mode, browsers, load and media, before a deploy or on a schedule |
| When | While working (the specs of what you touch) and before every commit | On a schedule and before a release ([who runs it](#who-runs-the-acceptance-suite)), and when asked |
| Who runs it | Every developer and agent, locally | The scheduled runner or a release step; an agent only when asked (below) |
| Where | `test/**/*.spec.js`, `test/client/**/*.spec.js` (as now) | `acceptance/**/*.spec.js` and `acceptance/client/**/*.spec.js` in each component, a separate tree |
| Command | `moqui test`, `moqui test-client`, `./moq test-apps`, `./moq test-framework`, `./moq test-all` | `./moq accept [component...]` (`moqui test --acceptance`, `moqui test-client --acceptance`); `./moq accept-all`, which also runs the Rust opt-in and ignored tests |
| Discovery | Never sees `acceptance/` | Only `acceptance/` |
| Size | A budget per component, enforced (below) | No budget; as complete as it needs to be, with one sweep per app and mode at most |
| Failure | Blocks the commit | Reported; a test that fails on two runs in a row is fixed or deleted; never blocks a commit |

- **Separate by location, not by a flag.** A spec is in one suite or the other by the directory it is in. No option of
  the regression commands pulls in acceptance tests, so they cannot leak into normal runs the way `slow` specs did when
  named or passed `--slow`. Rust has no such directory: its acceptance tests stay `#[ignore = "slow: ..."]` or opt-in
  `[[test]]`s, `./moq accept-all` runs them, and no regression command does.
- **What goes where.** A regression test checks one rule or a fixed bug: a service's result and refusals, a posting, a
  state change, the one render of a screen with logic of its own, one golden-path walk per app. An acceptance test checks
  a scenario or the system: a plan's "usable when" walk (server side through the screens' transitions, or in a
  browser), an access or isolation walk over every route and group, a sweep of every screen of an app in a mode, a
  browser walk of a flow, a load or media test. The acceptance suite may be large; its tests can be deleted freely,
  since nothing depends on them.
- **Shared data and fixtures.** Each suite loads its data once per run. Acceptance fixtures live under `acceptance/`
  and never in `test/`, so a regression run loads none of them; an acceptance run loads the regression fixtures as well
  (the users of `ClientFixtures` and the like), and serves its client specs as `--serve` does.
- **The rendering rule is the regression suite's.** "No test walks every screen" (AGENTS.md) holds for `test/`. A
  sweep has one place, `acceptance/`, and one per app per mode.

### When an agent runs the acceptance suite

Only:

1. when the user asks for it (a component's acceptance tests, one spec, or all), or
2. when the agent adds or changes an acceptance test: then it runs that test, and the acceptance suite of the component
   it is in, once the test is written.

Never because code they use changed, never to check its own work, and never as part of "done" for a phase. A plan or
brief that wants scenario checks names the acceptance tests to write; running them is the user's call.

### Who runs the acceptance suite

There is no CI today (only Java-era `.travis.yml` files in mr-mantle and mr-marble, which can go). Until there is,
`./moq accept-all` runs on a schedule on a spare machine (nightly, or weekly while it is long) and before a release,
and writes a dated summary with `--junit`. A test that fails on two runs in a row is fixed or deleted, and one that is
ignored or skipped on every run is deleted. Tests that no one runs rot: in the measured run the three call walks failed
by timeout and ForumHttp was ignored, and nothing asked anyone to look.

### The regression budget

- Each component declares its regression budget in seconds of feature time in a `test/budget` file, beside the specs
  it limits and changed in the same diff (not in `component.xml`, which is the server's configuration). Feature time is
  the serial sum, so parallel workers do not hide growth.
- Budgets are set from the measured run after the cut ([step 7](#order-of-work)), not before. Projected: mr-boss about
  220 s, mr-mantle about 210 s, webroot 26 s, mr-pass 15 s, the others under 12 s each.
- `moqui test` and `moqui test-client` warn when a component's feature time goes over its budget, and fail the run over
  1.5 times it or when one spec takes over 15 s. The margin keeps a slower machine from failing a run; the warning keeps
  the growth visible. That replaces today's end-of-run warning for specs over 30 s: a warning alone did not stop the
  growth.
- A new app adds its own budget, so test-all grows by a known amount per app rather than combinatorially.
- A budget is raised only with the user's agreement, like any other performance regression.

### The slow flag afterwards

Once the acceptance suite exists, `slow: true` goes: a long test belongs in the acceptance suite or is deleted. The
specs marked slow now (AppStructure, TuiRender, AccountingAccessWalk, the ServerStatic family, the three call walks,
mr-pass ForumHttp and Scale) move to `acceptance/` in step 0b; the call walks are fixed or deleted there. `--slow` goes
from the spec commands. The slow-test instructions shrink to one line: a regression spec over 15 s fails the run; move
it to acceptance or delete it. For Rust, which `cargo test` does not time: a test over 15 s is `#[ignore]` or opt-in,
and so an acceptance test.

### Cheaper machinery

Besides the content, the run itself wastes time. With the rule tests kept, these are what bring test-all under 10
minutes, so they come right after the A, B and C cut ([step 4](#order-of-work)), not later:

- **Parallel workers.** Specs of independent data run in workers, each on its own database copy taken from the
  `--db snapshot` (`crates/moqui-host/src/test_cmd/fixture.rs`), ordered by the specs' `dependsOn`. On 4 workers
  `moqui test` goes from about 8½ minutes to about 2¼.
- **A profile of the per-feature cost.** A server feature takes 0.22 s on average where a render takes milliseconds.
  Find where it goes (setup per feature, transactions, data made and removed) before cutting any rule test for time.
- **Fixtures once.** A fixture spec runs in plain `moqui test` and again on its served server; SimpleScreensRender, and
  the eight Mantle flows its `dependsOn` names, run twice per test-all the same way. A fixture runs only where a served
  suite names it (`server.specs` in `test/client/suite.js`).
- **A snapshot that survives a relink.** The `--db snapshot` key includes the binary's size and mtime (`fixture.rs`),
  so every relink throws away all the served snapshots and each server loads its data again. Key it on the data files
  and the schema.
- **One server for all served client specs** of a run instead of one per server group (seven now, six after the cut).
  It saves the server starts, about 15 s each, not the browser page each spec opens.
- **Builds apart.** `test-apps` builds the binary only when it is missing (as `--nobuild`) and never runs `cargo test`.
  A source change compiles both the dev profile (`cargo test`) and the release binary, and the opt-in host tests
  (`-p moqui-host`) resolve their features apart and build a third set, one more reason they belong to `accept-all`.
  sccache for all of them.

## Instructions to update

The tests grew because the instructions asked for them, so the cut does not hold unless the instructions change with it.
The inventory below is every place that says which tests to write or run, as of Nov 4, 2026 (paths in mr-moqui unless
a component is named). Each row names its section, which holds where a line number drifts (one merge moved
`docs/Build-Test.md` by 86 lines). Rules marked **growth** made the suite grow directly.

### The new test-writing rules (the text the edits put in place)

- **Two suites.** The regression suite (`test/`) is for development time: small, run while working and before every
  commit. The acceptance suite (`acceptance/`) is for pre-deploy and system validation: complete scenarios, run on a
  schedule or when asked. Say which suite a new test goes in, and why.
- **Regression tests check rules, not pages.** Write one for a business rule you add or change (its result and its
  refusals) and for a bug you fix (the test that would have caught it). Access: one holder and one non-holder on one verb
  of the rule, and another organization's record refused; not every route for every group. A screen is rendered in a
  regression test only when it has logic of its own, once per mode, with all of its checks on that render, and in a
  second mode only where it has its own qvt or tui template. No `where:` doubling of authorization on and off, no users ×
  modes matrices. Mantle rules are tested in Mantle, not again through an app. A rule test is not cut to meet a count:
  time decides, through the budget.
- **Walks.** The regression suite has one golden-path client walk per app, kept current. Every other walk is an
  acceptance test: a plan's "usable when" scenario (server side or in a browser), a phone-width check, an access or
  isolation walk.
- **Sweeps.** The acceptance suite may hold one screen sweep per app per mode; the regression suite holds none.
- **Budget.** A regression spec runs in well under a second as a rule; one over 15 s fails the run. A component over
  1.5 times its budget fails the run; cut duplicates or move scenarios to acceptance rather than raise the budget.
- **What to run.** While working: the regression specs of what you touched (`./moq test <component> --spec ...`).
  Before a commit: `./moq test-apps` for application work, `./moq test-framework` for framework work; `test-all` only
  before a release or when asked. The acceptance suite only when asked, or when you add or change an acceptance test (that
  test, then its component's acceptance suite).
- **Plans and Done.** A phase is done when its regression tests for the rules it added pass, the component's regression
  suite passes within budget, and `moqui lint --screens` is clean. A plan's "usable when" scenarios become acceptance
  tests to write (named in the plan), not regression tests, and running them is not part of Done.

### Where the edits go

**mr-moqui**

| Where | Now | Change |
|---|---|---|
| `AGENTS.md` slow-tests item (line 5) | Never run a slow test without permission; over 30 s marked slow; slow tests are deletion candidates | Replace with the two suites: their purpose, the line for when an agent runs acceptance tests, the regression budget. The slow rule shrinks to one line (a regression spec over 15 s fails the run; a Rust test over 15 s is `#[ignore]` or opt-in) |
| `AGENTS.md` rendering item (line 6) | No full walks, one render per mode | Keep, scoped to the regression suite; acceptance holds one sweep per app per mode at most |
| `AGENTS.md` (missing) | Nothing says what to run before a commit | Add: `test-apps` for app work, `test-framework` for framework work, never `test-all` or `cargo test` for app work unless asked |
| `docs/Build-Test.md` "The moq script" (423) and "Rust tests and lints" (452-456) | test-all and `--slow` | The commands of both suites (`test-apps`, `test-framework`, `test-all`, `accept`, `accept-all`); `accept-all` runs the Rust opt-in and ignored tests |
| `docs/Build-Test.md` "Slow tests" (458-475) | The slow rules | Becomes "The two suites" and "The regression budget"; `slow` a short note |
| `docs/Build-Test.md` "Rendering screens in tests" (477-488) | No full walks, no screenshots | Keep, under the regression rules, scoped as in AGENTS.md |
| `docs/Build-Test.md` "Running" (525), `spec({...})` (606-607), `feature(...)` (619) | `--slow` and `slow: true` in the option and spec reference | `--acceptance`; `slow` removed |
| `docs/dev/Practices-App-Dev.md` §1 "Commands" (72-73) | `./moq test`, `test-client --serve` | Add `test-apps` and `accept` |
| `Practices-App-Dev.md` §3 (112-113) | Each Mantle rule gets its own spec | Keep, worded as a rule test in Mantle |
| `Practices-App-Dev.md` §7 "Tests" (363-421) | No full walks; error-text checks; **growth:** a spec with a positive and a negative user for every service with an access rule; absence checks with a presence check; run the whole component before a commit; "Keep a spec fast" (the slow rules); "A test earns its place"; **growth:** phone layout checked at 390 px; **growth:** walks for scenarios a server spec cannot check, and a plan's scenario checked by a server spec through the screens' transitions | Rewrite to the new rules above: access checks per rule, not per service; "Keep a spec fast" becomes the 15 s limit and the budget; "A test earns its place" stays; the 390 px walks, the scenario walks and a plan's scenario (server side or browser) go to acceptance; before a commit, `test-apps` |
| `Practices-App-Dev.md` §8 (435) | Commit per phase naming the tests run | Keep |
| `Practices-App-Dev.md` §9 checklist (447, 453) | "the screen renders in tui"; **growth:** "`./moq test` for the app and Mantle ... and the walks run" | The regression suite within budget and lint; no walks |
| `docs/dev/Client-Testing.md` "Running" (54), "`--serve`" (124), "Files and order" (186) | `--slow` and specs over 30 s | `--acceptance`, `acceptance/client/`, the 15 s limit |
| `docs/dev/Practices-Rust.md` §3 (119-122), §4 (138-139), §9 (370) | Slow Rust tests ignored or opt-in; `cargo test -p` per crate touched | Keep `cargo test -p` per crate; the slow Rust tests (`cargo test-slow`, the opt-in host tests) are the framework's acceptance tests, run by `accept-all`; a Rust test over 15 s is `#[ignore]` or opt-in |
| `docs/ui/TUI.md` "Themes" (346-348) | Gallery snapshots of every screen (`tui_screens`, opt-in) | An acceptance test of the framework |
| `docs/ui/Desktop-Client.md` "Tests" (418-432), `Qvt-Client.md` (25) | Which commands cover what; ShellComms and ShellCalls by name | The suite each belongs to; ShellComms and ShellCalls are deleted |
| `docs/tools/Comms.md` 262, `docs/interfaces/Voicemail.md` 214 | `BasicComms`: "every screen in qvt and tui" | What BasicComms checks after its sweep render goes |
| `docs/ai/Assist.md` 756 | Shell shots in light and dark at 1280 and 390 px | No screenshots (already against the rule) |
| `plans/Comms-Framework.md` 345 | **growth:** "The `AppStructure`-style render covers every BasicComms screen in qvt ... and in tui" | Drop it: BasicComms drops its every-screen render (group F) |
| `plans/Comms-Framework.md` 1318-1327 | Browser, interop and slow runs only with permission | The acceptance suite, with its line |
| `plans/Crate-Split.md` 217-218, `plans/Autodoc.md` 318-320 | A phase checked by `./moq test-all`; a Chromium check (gate3, slow, with permission) and a look at 390 px | `test-framework` for the phase check; the Chromium check is an acceptance test |
| `plans/design-dev-aide.md` 1853-1854 | **growth:** Chromium smokes per phase, with light, dark and 390 px screenshots | Acceptance tests; no screenshots |
| `runtime/dev-component/template/test/@Class@Screens.spec.js` | **growth:** renders both template screens in qvt and tui (EditItem in tui twice), so every new component starts with an every-mode render test | Replace with one example rule spec in `test/`, and move a screen sweep example to `acceptance/` |
| `runtime/dev-component/template/README.md` 16, 24; `runtime/dev-component/README.md` 32-42 | `test/` covers services, screens and REST; run `./moq test` | The two suites and their commands |
| `runtime/dev-component/demo/README.md` 60-61, `example/README.md` 36-38 | Demo renders each stubbed screen in qvt and tui, run by test-all; Gallery renders every gallery screen, GallerySmoke again in the shell | Demo without its loops; the gallery sweep in acceptance |
| `runtime/base-component/basic-comms/README.md` 29, `framework/test/client/README.md` 9-12, `webroot/screen/webroot/js/README.md` 8-10 and 90-91, `runtime/dev-component/dev-aide/README.md` 47-52 | Run commands; the Gate 3 smoke; dev-aide's screenshots in light, dark and 390 px | Both suites; no screenshots |

**mr-boss** (`plan/` unless noted)

| Where | Now | Change |
|---|---|---|
| `impl-projects.md` §2 Done (47-50), which the later plans inherit | **growth:** AppStructure covering new screens, client walks passing, the phase's walk | The new Done: the phase's rule tests, the regression suite within budget, lint |
| `impl-drawers-commons.md` §4 Done (150-157), §7 (690-697) | **growth:** AppStructure extended to every Commons and Comms screen, the client walks; per-milestone "Specs:" | The same new Done; walks per milestone become acceptance |
| `impl-accounting.md` §2 Done (204-219), §8 (1545-1561), P21 (881-886) | **growth:** AppStructure, client walks, AccountingAccess extended every phase, 390 px for PASS (P18); the full access walk | New Done; AccountingAccess per rule; the access walk and §8 walks are acceptance |
| `impl-warehouse.md` §2 Done (165-177), P18 (809-814), §8 | **growth:** AppStructure, walks with handheld at 390, WarehouseAccess every phase | Same |
| `impl-manufacturing.md` §2 Done (191-202), the "Specs" lists (303-306, 490-493, 599-603, 876-885), P17 (2666-2673), §8 | **growth:** AppStructure and TuiRender routes, a ManufacturingAccess section per phase, a client walk per phase at 390 | Same; the spec lists become rule tests plus named acceptance tests |
| `impl-products.md` §2 Done (119-130), 83-84, P14 (623-627), §8 | **growth:** AppStructure, walks, ProductsAccess every phase, the ProductsPhone pattern | Same |
| `impl-customer-service.md` §2 Done (267-282), §8 (1182, 1198-1200) | **growth:** AppStructure and TuiRender, walks, the access spec every phase; screenshots light and dark, PASS at two widths | Same; screenshots are dropped (already against the no-screenshot rule) |
| `impl-purchasing.md` §2 Done (230-245), §8 (1032, 1050-1052) | **growth:** AppStructure (no TuiRender), walks, PurchasingAccess every phase; screenshots | Same |
| Every `impl-*.md`: P1's "Specs." block (accounting 317, customer-service 410, products 182, purchasing 347, warehouse 268, manufacturing 303) and each phase's "Usable when" | **growth:** P1 defines the Shell and Access specs that each later phase extends through Done; usable-when scenarios that §8 turns into tests | "Usable when" names acceptance tests; "Specs." names the rule tests only |
| `design-marketing.md` 1827-1829, `design-sales.md` 1723-1725, `design-maintenance.md` 727, `design-insights.md` 760, `design-admin.md` 1105 | **growth:** every screen in qvt and tui, 390 px, new routes joining the structure and TUI specs, a walk of each flow: the seed of the usable-when lists of apps not built yet | Acceptance tests by name; no structure or TUI routes |
| `review-impl-core-apps.md` 141, 216; `review-impl-core-apps/crosscut.md` 280-301 | **growth:** TuiRender in each phase's Done; each P1 adds its root to TuiRender; a shared Done with AppStructure, TuiRender, walks and shots at 390 and 1280 | Marked as superseded by the new Done: review records, but they read as rules |
| `docs/ManufacturingImplementation.md` 111-123, `docs/WarehouseImplementation.md` 1071-1072, `docs/AccountingImplementation.md` 529-543 | **growth:** each new screen adds its route to the route fixtures (AppStructure, TuiRender, ServerStatic); the run commands and walks; "AppStructure and TuiRender render every Accounting screen" | The suites by name; no route fixtures |
| `style-guide.md` 933-937 | **growth:** a new XML element asks for goldens, the Gate 3 browser smoke, terminal snapshots and a gallery screen | Goldens and a rule test; the browser smoke and the snapshots are acceptance |
| `style-guide.md` 1022-1023 | Gallery.spec.js and GallerySmoke render every gallery screen | The gallery sweep, an acceptance test of the framework |
| `README.md` 10-12 | Spec layout and the run commands | Both suites |

`design-doc-guide.md`'s "Stub spec" is a short specification of a framework need, not a test: no change there.

**mr-mantle, mr-pass, mr-marble**

| Where | Now | Change |
|---|---|---|
| mr-mantle `README.md` 27-28, `test/README.md` 10-16 | Run commands | Both suites |
| mr-mantle `plan/payments-shipping-edi.md` 385-390 | Fast specs; sandbox and live calls are deployment exercises | Already the split: the sandbox and live calls are acceptance tests |
| mr-pass `README.md` 39-44, `docs/Deployment.md` 310-360 | Spec lists, including `ForumHttp --slow`; the RemoteHttp commands | ForumHttp, Scale and RemoteHttp in `acceptance/` |
| mr-pass `plan/design-forum.md` §19.1 (1528-1557), `design-shop.md` 83, 1604 | Acceptance scenarios with `moqui test` and `test-client`; desktop and phone (forum), 390 px (shop) | The acceptance suite, by name |
| mr-marble `test/README.md` 24-31, 38, 63-67; `README.md` 30 | **growth:** SimpleScreensRender and MarbleScreens render every parameter-free screen; FindScreens every Find screen; BrowserSmoke | The sweeps move to acceptance; the regression suite keeps BrowserSmoke |

Also the briefs and skills agents are given: `.agents/skills/building/SKILL.md` says to run specs on an already built
binary and only the specs for the change, and nothing about the suites; a short `testing` skill (or a section of the
building one) carries the two suites and the line for agents, so a brief need not restate them. The worker and review
briefs of a build name the suite of each test they ask for.

## The run measured

The times are the per-feature times of one `./moq test-all` run (`.claude/worktrees/wave-logs/mfg-test-all.log`,
2026-11-04), summed per spec file. Every one of the 569 specs that ran maps to one file; the four `template` specs
(`@Class@*`, `SiteSmoke`) do not run.

| Stage | Wall | In features | Specs | Features |
|---|--:|--:|--:|--:|
| `cargo test --workspace` | 97 s of tests | | | 1,431 in moqui-core |
| opt-in host tests (gate3, client_browser, tui_screens) | 36 s + 4 min 19 s build | | | 13 |
| `scripts/moq-test.sh` | not logged | | | 321 |
| `moqui test` | 944 s (+ 2 min 15 s release build) | 881 s | 429 | 4,018 |
| `moqui test demo` | 15 s | 0.2 s | 1 | 56 |
| `moqui test-client webroot --offline` | 59 s | 33 s | 33 | 376 |
| `moqui test-client dev-aide --offline` | 7 s | 0.0 s | 7 | 44 |
| `moqui test-client --serve` (7 servers) | 1,014 s | 814 s | 99 | 425 |

Where the time is, besides the features: `moqui test` spends about 15 s loading the database and 0.15 s setting up each
spec (63 s over 429); `--serve` spends about 200 s starting servers and opening a browser per spec, 91 s of it on the
mr-boss server and its 69 walks. The opt-in host tests now run only with `--slow` (`moq`), which also adds
gate3_browser, `cargo test-slow` and the ignored `cli` tests; all of them become `accept-all`'s.

The 569 specs that ran are 429 in `moqui test` (410 specs and 19 fixture specs), 1 demo spec and 139 client specs (40
offline, 99 served), with 4,919 features. Below, fixture specs are not counted as specs; their time and features are.
Four specs were added after the run (basic-comms BasicCommsChatChanges, BasicCommsSync and client/ShellSync, framework
PushDevice); their rows show "—", and they are kept and not counted.

## Framework tests and app tests

Two kinds of tests, run for different work:

- **Framework tests** check MR Moqui itself: `cargo test`, `scripts/moq-test.sh`, the specs and client specs of
  `runtime/base-component` (framework, basic-comms, tools, webroot) and `runtime/dev-component` (example, dev-aide, demo).
  They need running only after a change to the framework: `crates/`, `runtime/base-component`, `runtime/dev-component`,
  `moq` or the scripts.
- **App tests** check the applications: the specs and client specs of the components under `runtime/component` (today
  mr-mantle, mr-boss, mr-pass and mr-marble). They are what work on an application runs.

Today nothing tells them apart: `moqui test` and `moqui test-client --serve` with no component take every component
under base-component, component and dev-component, and `./moq test-all` adds `cargo test` and the `moq` tests. So app
work runs the framework's tests too. In the Manufacturing build, each wave's client run (`test-client --serve`) also
ran basic-comms' call walks and the framework, webroot and tools client specs, and the final test-all spent about 10 min
on framework stages: the cargo test build and tests, the opt-in host tests, and the framework's specs and walks.

The change:

- **`./moq test-apps`** (new): `moqui test` and `moqui test-client --serve` over the components under
  `runtime/component` only. There is no cargo build or test, no `moq-test.sh`, and no base-component or dev-component
  spec. It builds the binary only when it is missing (as `--nobuild`). This is the command for application work, and
  the one `AGENTS.md` and `docs/dev/Practices-App-Dev.md` name for it.
- **`./moq test-framework`** (new): `cargo test`, `moq-test.sh`, and the specs and client specs of base-component and
  dev-component. This is for framework work.
- These are the regression suite; the acceptance suite has its own commands (`./moq accept`), split the same way by
  component.
- **`./moq test-all`** stays both in turn: before a release, after a framework change that apps depend on, or when
  asked.
- **The framework's own specs stay out of app runs:** a component spec run (`./moq test mr-boss`) already loads every
  loaded component's data but runs only that component's specs. `moqui test` with no component keeps its meaning
  (every component); `test-apps` is the narrower command.
- **Rule** (AGENTS.md): when only application components changed, run the app tests of the components touched while
  working, and `./moq test-apps` before a commit. Never run `cargo test`, the framework specs or test-all for
  application work unless asked.

Projected, with the cuts below (serial, and with `moqui test` on 4 workers):

| Run | Stages | Serial | 4 workers |
|---|---|--:|--:|
| `test-apps` | moqui test over 4 components: load about 15 s, 315 specs and fixture specs × 0.15 s, 412 s of features; `--serve` with 3 servers (mr-boss, mr-mantle and mr-marble, mr-pass), about 45 s to start, 6 specs, 37 s | about 9¼ min | about 3½ min |
| `test-framework` | cargo test about 1½ min; moq-test.sh (not measured); 51 specs and fixture specs of framework, basic-comms, tools, example, dev-aide, about 24 s; demo 15 s; 23 offline client specs (webroot 19, dev-aide 4), 26 s; 8 served (framework 5, basic-comms 2, example 1), 10 s plus 3 servers | about 4½ min | the same |
| `test-all` | both | about 13½ min | about 7¼ min |

## How the cut is made

- **Five decisions.** `keep`. `merge into X`: all of its checks move into X, to save a spec's setup, page or server,
  and nothing is dropped. `acceptance`: it moves to `acceptance/`, merged where several cover the same ground (one
  screen sweep per app in each mode it needs, one access and isolation walk per app). `delete`: a true duplicate, a
  render, user, mode or flag that a kept test already checks. `fixture`: data a kept walk needs.
- **Time decides, not counts.** There are no feature caps and no spec or feature targets: a feature count is not what
  costs time (service calls and data setup are), and a cap that squeezes many specs into four features deletes rules
  under the name of a merge. The multiplication goes: the walk-everything renders (A), the authorization doubling and
  the access walks (B), and the browser walks beyond one golden path per app (C). The rule tests (D, E, F) stay unless
  they duplicate another test; their time is the machinery's to cut.
- **One render per screen and mode.** Where two regression specs render a screen in the same mode, the spec that owns
  the screen's logic keeps the render and every check of it; an access spec checks refusals through service calls and
  transitions (or the status of a refused render), not by rendering the content again. The
  [duplicate renders](#duplicate-renders) table names the keeper for each.
- **More than one mode** only for a screen with its own qvt or tui template: mr-boss has 19 `.qvue` and 6 `.tui.json`,
  basic-comms 18 and 18, mr-marble 3 and 1, example 3 and 2, dev-aide 3 `.qvue`, mr-pass 2, webroot 1. The check says
  which template it covers. A `render-mode` section is a second mode only where the section differs.
- **Authorization on and off.** mr-mantle has no `where:` doubling: 175 of its 176 specs turn authorization off in
  their setup. The doubling is in mr-boss's access specs, whose `where: both` features come in "with authz true" and
  "with authz false" pairs: WarehouseAccess 30 pairs (60 of its 136 features), ManufacturingAccess 13 (26 of 94),
  AccountingAccess 13 (26 of 61), ProductsAccess 4 (8 of 24). Group B drops it: each access feature runs once, with
  authorization on.
- **Fixtures** stay only where a kept walk needs them: 11 of the 12 mr-boss fixture specs move to acceptance with their
  walks, which also shortens the mr-boss server start.

## Summary

By group (fixture specs are not counted as specs; their time and features are):

| Group | Specs now | Features now | Time now | Specs kept | To acceptance | Projected features | Projected time |
|---|--:|--:|--:|--:|--:|--:|--:|
| A. Walk-everything renders | 32 | 585 | 350 s | 1 | 21 | 12 | 0 s |
| B. Access specs | 18 | 426 | 162 s | 6 | 6 | 40 | 12 s |
| C. Client walks | 134 | 798 | 783 s | 36 | 73 | 593 | 74 s |
| D. mr-boss app specs | 124 | 1066 | 191 s | 113 | 9 | 1011 | 180 s |
| E. mr-mantle | 176 | 1574 | 210 s | 174 | 0 | 1564 | 209 s |
| F. Small components | 66 | 470 | 34 s | 64 | 2 | 468 | 34 s |
| **All** | **550** | **4919** | **1729 s** | **394** | **111** | **3688** | **509 s** |

By kind:

| Kind | Specs now | Features now | Feature time now | Specs kept | Projected features | Projected feature time |
|---|--:|--:|--:|--:|--:|--:|
| Server specs (`moqui test`, with their fixtures) | 410 | 4018 | 882 s | 357 | 3104 | 435 s |
| `moqui test demo` | 1 | 56 | 0 s | 1 | 12 | 0 s |
| Client specs, `--offline` | 40 | 420 | 33 s | 23 | 420 | 26 s |
| Client specs, `--serve` | 99 | 425 | 814 s | 13 | 152 | 47 s |

Groups A, B and C go from 1,295 s to 86 s of feature time. D, E and F keep their rules: 435 s to 423 s, the rest of
their time left to the machinery. In all, 394 specs stay, 111 move to acceptance, 33 merge into a kept spec and 12 are
deleted.

By component:

| Component | Stage | Specs now | Time now | Specs kept | To acceptance | Projected time |
|---|---|--:|--:|--:|--:|--:|
| basic-comms | client | 7 | 195 s | 1 | 4 | 2 s |
| basic-comms | server | 9 | 5 s | 9 | 0 | 5 s |
| demo | server | 1 | 0 s | 1 | 0 | 0 s |
| dev-aide | client | 7 | 0 s | 4 | 0 | 0 s |
| dev-aide | server | 5 | 12 s | 4 | 0 | 11 s |
| example | client | 2 | 17 s | 1 | 1 | 5 s |
| example | server | 7 | 3 s | 6 | 1 | 1 s |
| framework | client | 15 | 15 s | 5 | 2 | 3 s |
| framework | server | 24 | 6 s | 24 | 0 | 6 s |
| mr-boss | client | 69 | 502 s | 4 | 65 | 30 s |
| mr-boss | server | 161 | 617 s | 119 | 28 | 192 s |
| mr-mantle | server | 176 | 210 s | 174 | 0 | 209 s |
| mr-marble | client | 2 | 32 s | 1 | 1 | 4 s |
| mr-marble | server | 7 | 17 s | 2 | 5 | 0 s |
| mr-pass | client | 3 | 22 s | 1 | 2 | 4 s |
| mr-pass | server | 17 | 11 s | 16 | 1 | 11 s |
| tools | client | 1 | 30 s | 0 | 1 | 0 s |
| tools | server | 4 | 2 s | 3 | 0 | 2 s |
| webroot | client | 33 | 33 s | 19 | 0 | 26 s |

## Projected stage times

| Stage | Now | Serial | 4 workers | How |
|---|--:|--:|--:|---|
| `cargo test --workspace` | 97 s | about 1½ min | the same | unchanged; the three golden failures of the run are a separate fix |
| `moqui test` | 944 s | about 8½ min | about 2¼ min | 15 s load, 366 specs and fixture specs × 0.15 s, 435 s of features |
| `moqui test demo` | 15 s | 15 s | the same | |
| `--offline` (webroot, dev-aide) | 66 s | about ¾ min | the same | 23 specs × 0.8 s, 26 s of features |
| `--serve` | 1,014 s | about 2½ min | the same | 6 servers (tools has no client spec left) × about 15 s, 14 specs × 0.8 s, 47 s of features |
| `scripts/moq-test.sh` | not logged | not known | | measure it |
| **test-all, tests only** | **about 37 min** | **about 13½ min** | **about 7¼ min** | builds and `moq-test.sh` not counted |

A projected time is the spec's time scaled by the features it keeps (B's rewrites at 0.3 s a feature), plus a quarter
of the time of each spec merged into it; a spec added after the run counts nothing. The 10-minute goal needs the
parallel workers: serial, the kept rule tests alone are about 7 minutes of `moqui test`. The builds of the run measured
(a 7 min 53 s debug test build, 4 min 19 s for the opt-in host tests, now `accept-all`'s, and 2 min 15 s of release
build) are not counted.

## Order of work

Each step is one change, checked by running the specs of the area it touches:

0. **a.** `test-apps` and `test-framework`, and the instruction edits ([above](#instructions-to-update)).
   **b.** `acceptance/` discovery and `--acceptance` in `moqui test` and `moqui test-client`, `./moq accept` and
   `accept-all` (with the Rust opt-in and ignored tests); then the specs marked slow move to `acceptance/`, and
   `--slow` goes.
1. Group A: the deletes, and the sweeps moved to acceptance, one per app per mode. Before SimpleScreensRender moves,
   take it out of `server.specs` in `mr-marble/test/client/suite.js`: BrowserSmoke's server runs it there, and through
   its `dependsOn` the eight Mantle flows. Check that BrowserSmoke needs nothing those write.
2. Group C: the walks and their fixtures to acceptance; the webroot, dev-aide and security merges.
3. Group B: the six access rewrites, one per app; the access and isolation walks to acceptance.
4. Machinery: parallel workers on snapshot copies, fixtures run once, a snapshot key that survives a relink, one server
   for a run's served specs, and a profile of the per-feature cost.
5. Group D, one app at a time: the Projects walks to acceptance, the duplicate renders, the checks of a Mantle rule that
   Mantle already tests, and Contacts under 15 s.
6. Groups E and F: the two Mantle deletes once their rules are found in OrderToCashBasicFlow; BasicComms without its
   every-screen render; ArtifactIndex's index built once.
7. Budgets: measure the run, write each component's `test/budget`, and turn on the warning at the budget, the failure
   at 1.5 times it, and the 15 s spec limit.

Projects and Commons have no client walk today, so they keep none; a golden-path walk for each is new work, not part of
this cut.

## A. Walk-everything renders

Every walk-everything render moves to acceptance, merged into one sweep per app per mode, or is deleted where it
repeats what a kept spec renders (the per-app Shell specs). Demo stays without its screen loops.

| Spec | Time | Features | Decision | Reason |
|---|--:|--:|---|---|
| `basic-comms/client/ShellCalls` | 0.5 | 2 | delete | desktop shell bundle renders; the desktop crate tests its bundle |
| `basic-comms/client/ShellComms` | 0.8 | 3 | delete | desktop shell bundle renders; the desktop crate tests its bundle |
| `demo/Demo` | 0.2 | 56 | keep, trim to 12 | the demo lock-down; drop the loops over every stubbed screen, keep one per rule |
| `dev-aide/DevAideScreens` | 1.2 | 9 | delete | Home and Settings render; the client specs cover the workspace |
| `example/Gallery` | 2.1 | 59 | acceptance | the gallery sweep: every gallery screen in its render mode |
| `example/client/GallerySmoke` | 12.3 | 23 | acceptance | the gallery sweep in the /qapps shell, with Gallery |
| `mr-boss/Accounting/AccountingScreens` | 10.8 | 3 | acceptance | the Accounting qvt sweep: every Accounting route |
| `mr-boss/Accounting/AccountingServerStatic` | 14.7 | 1 | acceptance | the Accounting server-static sweep; was marked slow |
| `mr-boss/Accounting/AccountingShell` | 3.9 | 45 | delete | per-app shell walk: launcher and menus render once in the kept specs' first page |
| `mr-boss/AppStructure` | 51.7 | 18 | acceptance | every app root and default screen: the roots of the app sweeps; was marked slow |
| `mr-boss/Manufacturing/ManufacturingServerStatic` | 7.0 | 1 | acceptance | the Manufacturing server-static sweep; was marked slow |
| `mr-boss/Manufacturing/ManufacturingShell` | 1.5 | 20 | delete | per-app shell walk: launcher and menus render once in the kept specs' first page |
| `mr-boss/Products/ProductsPriceScreens` | 5.6 | 12 | acceptance | into the Products sweep: the Prices screens |
| `mr-boss/Products/ProductsScreens` | 20.0 | 37 | acceptance | the Products sweep: every Products route |
| `mr-boss/Products/ProductsShell` | 1.4 | 13 | delete | per-app shell walk: launcher and menus render once in the kept specs' first page |
| `mr-boss/Products/ProductsTaxonomyScreens` | 29.2 | 58 | acceptance | into the Products sweep: the taxonomy screens, 29 s |
| `mr-boss/Products/ProductsVariantScreens` | 6.0 | 13 | acceptance | into the Products sweep: the variant screens |
| `mr-boss/Projects/ProjectsServerStatic` | 12.8 | 1 | acceptance | the Projects server-static sweep; was marked slow |
| `mr-boss/Projects/ProjectsShell` | 0.4 | 4 | delete | per-app shell walk: launcher and menus render once in the kept specs' first page |
| `mr-boss/Projects/ProjectsTui` | 5.4 | 7 | acceptance | the Projects tui sweep |
| `mr-boss/ServerStatic` | 31.6 | 1 | acceptance | the server-static sweep of Commons, Comms and Products; was marked slow |
| `mr-boss/Shell` | 4.6 | 4 | delete | the suite shell's IronStone part; shellData is the framework's and Commons' first render |
| `mr-boss/TuiRender` | 47.5 | 3 | acceptance | the tui sweep of Commons, Comms and Insights; was marked slow |
| `mr-boss/Warehouse/WarehouseServerStatic` | 6.7 | 1 | acceptance | the Warehouse server-static sweep; was marked slow |
| `mr-boss/Warehouse/WarehouseShell` | 1.6 | 25 | delete | per-app shell walk: launcher and menus render once in the kept specs' first page |
| `mr-marble/GitScreens` | 0.7 | 8 | acceptance | into the mr-marble sweep: the git screens |
| `mr-marble/MarbleScreens` | 3.9 | 18 | acceptance | into the mr-marble sweep: the dashboard and its SimpleScreens pages |
| `mr-marble/MyAccountScreens` | 0.7 | 18 | acceptance | into the mr-marble sweep: every My Account screen |
| `mr-marble/SimpleScreensRender` | 11.6 | 59 | acceptance | the mr-marble sweep (port of PopcAdminScreenTests); BrowserSmoke's server runs it today |
| `mr-marble/client/FindScreens` | 24.1 | 34 | acceptance | the mr-marble browser sweep: every Find screen in Chromium, 24 s |
| `tools/EdgeScreens` | 0.2 | 10 | delete | renders of the edge screens, 5 of 10 features skip without an edge node |
| `tools/client/Monitoring` | 29.6 | 19 | acceptance | a browser walk of the Monitor screens, 30 s; MonitorSettings keeps the services |
| **Subtotal** | **350 s** | **585** | **1 kept, 21 acceptance** | **projected 12 features, 0 s** |

## B. Access specs

One access spec per app, 6 to 8 features, each run once with authorization on: who may open the app, the organization
(or facility, plant, client) scope, one refused write, one refused read of another organization's record, and the
standalone case. The access and isolation walks move to acceptance, one per app. Projects' access spec is
ClientIsolation, renamed ProjectsAccess. Commons and Comms have no access spec of their own; AppOrganizationFilters
covers their organization filter.

| Spec | Time | Features | Decision | Reason |
|---|--:|--:|---|---|
| `mr-boss/Accounting/AccountingAccess` | 14.5 | 61 | keep, rewrite to 7 | the Accounting access spec; drops the authz true/false doubling (13 pairs) |
| `mr-boss/Accounting/AccountingAccessWalk` | 43.4 | 15 | acceptance | the Accounting access walk; was marked slow, 43 s |
| `mr-boss/Accounting/AccountingIsolation` | 0.5 | 4 | merge into AccountingAccess | organization isolation |
| `mr-boss/AppOrganizationFilters` | 0.2 | 25 | keep, trim to 6 | the user organization filter across the apps: one case per app |
| `mr-boss/Manufacturing/ManufacturingAccess` | 15.3 | 94 | keep, rewrite to 7 | the Manufacturing access spec; drops the doubling (13 pairs) |
| `mr-boss/Manufacturing/ManufacturingAccessWalk` | 17.4 | 5 | acceptance | the Manufacturing access and isolation walk, 17 s |
| `mr-boss/Manufacturing/ManufacturingCostIsolationWalk` | 8.0 | 2 | acceptance | into the Manufacturing access and isolation walk: cost isolation, 8 s |
| `mr-boss/Manufacturing/ManufacturingIsolationWalk` | 16.8 | 3 | acceptance | into the Manufacturing access and isolation walk, 17 s |
| `mr-boss/Products/ProductsAccess` | 1.5 | 24 | keep, rewrite to 6 | the Products access spec; drops the doubling (4 pairs) |
| `mr-boss/Projects/ClientAccessInternal` | 2.6 | 3 | merge into ClientIsolation | internal users unchanged by client access |
| `mr-boss/Projects/ClientChatDrive` | 0.8 | 12 | merge into ClientIsolation | client chat and drive scope |
| `mr-boss/Projects/ClientCode` | 2.2 | 5 | merge into ClientIsolation | shared repositories for clients |
| `mr-boss/Projects/ClientIsolation` | 5.8 | 16 | keep, rewrite to 7 | becomes ProjectsAccess: client isolation is Projects' access model |
| `mr-boss/Projects/ClientWalk` | 0.7 | 7 | merge into ClientIsolation | the client's walk renders Home, invoices and RequestDetail again |
| `mr-boss/Projects/ClientWrites` | 0.5 | 6 | merge into ClientIsolation | the client's own writes |
| `mr-boss/Warehouse/WarehouseAccess` | 11.2 | 136 | keep, rewrite to 7 | the Warehouse access spec; drops the doubling (30 pairs) |
| `mr-boss/Warehouse/WarehouseAccessWalk` | 11.7 | 4 | acceptance | the Warehouse access and isolation walk: the organization user's walk, 12 s |
| `mr-boss/Warehouse/WarehouseIsolationWalk` | 8.7 | 4 | acceptance | into the Warehouse access and isolation walk, 9 s |
| **Subtotal** | **162 s** | **426** | **6 kept, 6 acceptance** | **projected 40 features, 12 s** |

## C. Client walks

mr-boss keeps one golden-path walk per app that has one (Accounting, Products, Warehouse, Manufacturing); every other
mr-boss walk, with its fixture, moves to acceptance. basic-comms keeps CryptoFlow, and ShellSync, which needs no server;
mr-marble, mr-pass and example keep one walk each. The webroot, dev-aide and framework security specs are rule tests:
they keep all their features, and merge only where it saves a page or a server (about 0.8 s a spec).

| Spec | Time | Features | Decision | Reason |
|---|--:|--:|---|---|
| `basic-comms/CallClientFixtures` | 0.1 | 1 | acceptance | fixture of the call walks; moves with them |
| `basic-comms/CryptoClientFixtures` | 0.0 | 1 | fixture | the room CryptoFlow needs |
| `basic-comms/client/CallFlow` | 61.2 | 1 | acceptance | fake-media call; FAILED by timeout (61 s) in the measured run: fix or delete |
| `basic-comms/client/CallGuestFlow` | 60.7 | 1 | acceptance | fake-media guest call; FAILED by timeout (61 s): fix or delete |
| `basic-comms/client/CryptoFlow` | 1.7 | 1 | keep | the one non-media walk: crypto devices in two browsers, 1.7 s |
| `basic-comms/client/EncryptedCallFlow` | 61.4 | 1 | acceptance | fake-media SFrame call; FAILED by timeout (61 s): fix or delete |
| `basic-comms/client/EncryptedChatFlow` | 8.4 | 7 | acceptance | an encrypted room walk, 8 s; basic-comms keeps one walk (CryptoFlow) |
| `basic-comms/client/ShellSync` | — | — | keep | the shell's sync states, retries and conflicts (MD5 exit checks); no server |
| `dev-aide/client/DevAide` | 0.0 | 14 | keep | the Workspace client |
| `dev-aide/client/DevAideBrowser` | 0.0 | 6 | merge into DevAide | the Artifact Browser layout |
| `dev-aide/client/DevAideEntity` | 0.0 | 6 | keep | the entity operation builders, shared vectors with Rust |
| `dev-aide/client/DevAideGit` | 0.0 | 4 | merge into DevAide | workspace git depth |
| `dev-aide/client/DevAideQvue` | 0.0 | 4 | merge into DevAideScreen | the .qvue design view |
| `dev-aide/client/DevAideScreen` | 0.0 | 6 | keep | the screen view and block tree |
| `dev-aide/client/DevAideService` | 0.0 | 4 | keep | the service writer and logic composer |
| `example/client/HandheldScan` | 5.0 | 3 | keep | the one example walk: handheld density at 390 px |
| `framework/ClientFixtures` | 0.1 | 16 | fixture | the users the framework client specs sign in as |
| `framework/client/LoadBasic` | 10.2 | 3 | acceptance | a 10 s load run with no assertion beyond 0 errors |
| `framework/client/SecurityAccess` | 0.5 | 13 | keep | A01 over HTTP, the core of the security proofs |
| `framework/client/SecurityAuthn` | 0.1 | 9 | keep | A07 authentication |
| `framework/client/SecurityAuthnMore` | 0.8 | 21 | merge into SecurityAuthn | the A07 cases SecurityAuthn lacks |
| `framework/client/SecurityCors` | 0.0 | 8 | merge into SecurityHeaders | CORS |
| `framework/client/SecurityErrors` | 0.0 | 1 | merge into SecurityHeaders | A10 |
| `framework/client/SecurityHeaders` | 0.1 | 5 | keep | A02 headers |
| `framework/client/SecurityMessages` | 0.0 | 8 | merge into SecurityAccess | /rest/sm access |
| `framework/client/SecurityMisc` | 0.2 | 9 | merge into SecurityHeaders | leftover A01/A05/A10 proofs |
| `framework/client/SecurityProxy` | 0.1 | 7 | merge into SecurityAccess | proxy access |
| `framework/client/SecurityRest` | 0.9 | 22 | keep | A01 for REST |
| `framework/client/SecurityScreens` | 1.3 | 25 | keep | A01 for screens |
| `framework/client/SecurityStatus` | 0.0 | 3 | merge into SecurityAccess | /status access |
| `framework/client/SecurityTarpit` | 1.0 | 2 | acceptance | skips unless the load-test conf is on: a load-test case |
| `framework/client/SecurityWebSocket` | 0.1 | 4 | merge into SecurityAccess | /notws access |
| `mr-boss/Accounting/AccountingClientFixtures` | 0.0 | 1 | fixture | the users the Accounting walk signs in as |
| `mr-boss/Manufacturing/ManufacturingPlanningFixtures` | 0.1 | 1 | acceptance | fixture of PlanningWalk; moves with it |
| `mr-boss/Manufacturing/ManufacturingScheduleFixtures` | 0.5 | 1 | acceptance | fixture of ScheduleWalk; moves with it |
| `mr-boss/Manufacturing/ManufacturingShopFloorFixtures` | 0.4 | 1 | acceptance | fixture of ShopFloorWalk; moves with it |
| `mr-boss/Manufacturing/ManufacturingSubcontractFixtures` | 0.1 | 1 | acceptance | fixture of SubcontractWalk; moves with it |
| `mr-boss/Products/ProductsCostFixtures` | 0.4 | 1 | acceptance | fixture of CostWalk; moves with it |
| `mr-boss/Products/ProductsImportFixtures` | 0.1 | 1 | acceptance | fixture of ImportWalk; moves with it |
| `mr-boss/Products/ProductsInventoryFixtures` | 0.0 | 1 | acceptance | fixture of InventoryWalk; moves with it |
| `mr-boss/Products/ProductsMethodFixtures` | 0.4 | 1 | acceptance | fixture of MethodWalk; moves with it |
| `mr-boss/Products/ProductsPriceFixtures` | 0.0 | 1 | acceptance | fixture of PriceWalk; moves with it |
| `mr-boss/Products/ProductsToolsFixtures` | 0.0 | 1 | acceptance | fixture of ToolsWalk; moves with it |
| `mr-boss/Warehouse/WarehouseProductionFixtures` | 0.4 | 1 | acceptance | fixture of ProductionWalk; moves with it |
| `mr-boss/client/Accounting/AccountingBankingWalk` | 19.6 | 3 | acceptance | a scenario walk; the server spec keeps the rule |
| `mr-boss/client/Accounting/AccountingBudgetsWalk` | 10.4 | 3 | acceptance | a scenario walk; the server spec keeps the rule |
| `mr-boss/client/Accounting/AccountingCloseWalk` | 10.5 | 1 | acceptance | a scenario walk; the server spec keeps the rule |
| `mr-boss/client/Accounting/AccountingCollectionsWalk` | 6.2 | 2 | acceptance | a scenario walk; the server spec keeps the rule |
| `mr-boss/client/Accounting/AccountingCurrencyWalk` | 4.7 | 1 | acceptance | a scenario walk; the server spec keeps the rule |
| `mr-boss/client/Accounting/AccountingFixedAssetsWalk` | 4.5 | 1 | acceptance | a scenario walk; the server spec keeps the rule |
| `mr-boss/client/Accounting/AccountingLedgerWalk` | 7.7 | 1 | acceptance | a scenario walk; the server spec keeps the rule |
| `mr-boss/client/Accounting/AccountingMatchingWalk` | 5.9 | 1 | acceptance | a scenario walk; the server spec keeps the rule |
| `mr-boss/client/Accounting/AccountingPayablesWalk` | 10.6 | 1 | acceptance | a scenario walk; the server spec keeps the rule |
| `mr-boss/client/Accounting/AccountingPaymentGatewayWalk` | 7.7 | 1 | acceptance | a scenario walk; the server spec keeps the rule |
| `mr-boss/client/Accounting/AccountingPaymentRunsWalk` | 9.6 | 2 | acceptance | a scenario walk; the server spec keeps the rule |
| `mr-boss/client/Accounting/AccountingPaymentsWalk` | 13.0 | 1 | acceptance | a scenario walk; the server spec keeps the rule |
| `mr-boss/client/Accounting/AccountingReceivablesWalk` | 9.3 | 1 | keep | the app's golden-path walk: invoice, send, payment received |
| `mr-boss/client/Accounting/AccountingReportsWalk` | 6.0 | 3 | acceptance | a scenario walk; the server spec keeps the rule |
| `mr-boss/client/Accounting/AccountingReturnsWalk` | 3.5 | 1 | acceptance | a scenario walk; the server spec keeps the rule |
| `mr-boss/client/Accounting/AccountingSettingsWalk` | 12.0 | 1 | acceptance | a scenario walk; the server spec keeps the rule |
| `mr-boss/client/Accounting/AccountingSettlementWalk` | 11.9 | 1 | acceptance | a scenario walk; the server spec keeps the rule |
| `mr-boss/client/Accounting/AccountingSetupWalk` | 6.8 | 1 | acceptance | a scenario walk; the server spec keeps the rule |
| `mr-boss/client/Accounting/AccountingTaxWalk` | 16.8 | 3 | acceptance | a scenario walk; the server spec keeps the rule |
| `mr-boss/client/Accounting/AccountingValuationWalk` | 5.4 | 1 | acceptance | a scenario walk; the server spec keeps the rule |
| `mr-boss/client/Manufacturing/ManufacturingChangesWalk` | 3.2 | 4 | acceptance | a scenario walk; the server spec keeps the rule |
| `mr-boss/client/Manufacturing/ManufacturingCostCloseWalk` | 1.9 | 4 | acceptance | a scenario walk; the server spec keeps the rule |
| `mr-boss/client/Manufacturing/ManufacturingCostsWalk` | 2.4 | 3 | acceptance | a scenario walk; the server spec keeps the rule |
| `mr-boss/client/Manufacturing/ManufacturingDowntimeWalk` | 5.5 | 2 | acceptance | a scenario walk; the server spec keeps the rule |
| `mr-boss/client/Manufacturing/ManufacturingMaterialsWalk` | 1.6 | 2 | acceptance | a scenario walk; the server spec keeps the rule |
| `mr-boss/client/Manufacturing/ManufacturingMethodsWalk` | 7.1 | 5 | acceptance | a scenario walk; the server spec keeps the rule |
| `mr-boss/client/Manufacturing/ManufacturingPlanningWalk` | 4.1 | 3 | acceptance | a scenario walk; the server spec keeps the rule |
| `mr-boss/client/Manufacturing/ManufacturingPolicyWalk` | 6.6 | 2 | acceptance | a scenario walk; the server spec keeps the rule |
| `mr-boss/client/Manufacturing/ManufacturingQualityWalk` | 2.2 | 3 | acceptance | a scenario walk; the server spec keeps the rule |
| `mr-boss/client/Manufacturing/ManufacturingQuantitiesWalk` | 2.1 | 3 | acceptance | a scenario walk; the server spec keeps the rule |
| `mr-boss/client/Manufacturing/ManufacturingReportsWalk` | 2.1 | 3 | acceptance | a scenario walk; the server spec keeps the rule |
| `mr-boss/client/Manufacturing/ManufacturingScheduleWalk` | 2.8 | 3 | acceptance | a scenario walk; the server spec keeps the rule |
| `mr-boss/client/Manufacturing/ManufacturingShopFloorWalk` | 19.8 | 2 | acceptance | a scenario walk; the server spec keeps the rule |
| `mr-boss/client/Manufacturing/ManufacturingSubcontractWalk` | 6.0 | 4 | acceptance | a scenario walk; the server spec keeps the rule |
| `mr-boss/client/Manufacturing/ManufacturingWorkCentersWalk` | 8.4 | 3 | acceptance | a scenario walk; the server spec keeps the rule |
| `mr-boss/client/Manufacturing/ManufacturingWorkOrdersWalk` | 3.1 | 2 | keep | the app's golden-path walk: new work order to released |
| `mr-boss/client/Products/ProductsCatalogSettings` | 2.3 | 1 | acceptance | a scenario walk; the server spec keeps the rule |
| `mr-boss/client/Products/ProductsChecks` | 2.3 | 1 | acceptance | a scenario walk; the server spec keeps the rule |
| `mr-boss/client/Products/ProductsCostWalk` | 7.5 | 1 | acceptance | a scenario walk; the server spec keeps the rule |
| `mr-boss/client/Products/ProductsDocumentWalk` | 9.5 | 2 | acceptance | a scenario walk; the server spec keeps the rule |
| `mr-boss/client/Products/ProductsFinish` | 3.0 | 3 | acceptance | a scenario walk; the server spec keeps the rule |
| `mr-boss/client/Products/ProductsImportWalk` | 4.4 | 2 | acceptance | a scenario walk; the server spec keeps the rule |
| `mr-boss/client/Products/ProductsInventoryWalk` | 6.7 | 2 | acceptance | a scenario walk; the server spec keeps the rule |
| `mr-boss/client/Products/ProductsMethodWalk` | 21.2 | 1 | acceptance | a scenario walk; the server spec keeps the rule |
| `mr-boss/client/Products/ProductsPhone` | 8.0 | 9 | acceptance | the 390 px phone walk of Products |
| `mr-boss/client/Products/ProductsPriceListWalk` | 6.3 | 1 | acceptance | a scenario walk; the server spec keeps the rule |
| `mr-boss/client/Products/ProductsPriceWalk` | 10.1 | 2 | acceptance | a scenario walk; the server spec keeps the rule |
| `mr-boss/client/Products/ProductsSupplierWalk` | 8.1 | 2 | acceptance | a scenario walk; the server spec keeps the rule |
| `mr-boss/client/Products/ProductsTaxonomyWalk` | 8.8 | 1 | acceptance | a scenario walk; the server spec keeps the rule |
| `mr-boss/client/Products/ProductsToolsWalk` | 10.0 | 4 | acceptance | a scenario walk; the server spec keeps the rule |
| `mr-boss/client/Products/ProductsVariantWalk` | 6.7 | 2 | acceptance | a scenario walk; the server spec keeps the rule |
| `mr-boss/client/Products/ProductsWalk` | 7.7 | 1 | keep | the app's golden-path walk: editor form submit, lifecycle, child navigation |
| `mr-boss/client/Warehouse/WarehouseAdjustmentsWalk` | 4.7 | 4 | acceptance | a scenario walk; the server spec keeps the rule |
| `mr-boss/client/Warehouse/WarehouseCountsWalk` | 2.7 | 3 | acceptance | a scenario walk; the server spec keeps the rule |
| `mr-boss/client/Warehouse/WarehouseFacilitiesWalk` | 16.0 | 4 | acceptance | a scenario walk; the server spec keeps the rule |
| `mr-boss/client/Warehouse/WarehouseFoundationWalk` | 3.3 | 3 | acceptance | a scenario walk; the server spec keeps the rule |
| `mr-boss/client/Warehouse/WarehouseInspectionWalk` | 7.3 | 3 | acceptance | a scenario walk; the server spec keeps the rule |
| `mr-boss/client/Warehouse/WarehouseInventoryWalk` | 5.1 | 4 | acceptance | a scenario walk; the server spec keeps the rule |
| `mr-boss/client/Warehouse/WarehouseMovesWalk` | 5.8 | 4 | acceptance | a scenario walk; the server spec keeps the rule |
| `mr-boss/client/Warehouse/WarehousePackingWalk` | 11.6 | 4 | acceptance | a scenario walk; the server spec keeps the rule |
| `mr-boss/client/Warehouse/WarehousePickingWalk` | 3.4 | 3 | acceptance | a scenario walk; the server spec keeps the rule |
| `mr-boss/client/Warehouse/WarehousePoliciesWalk` | 8.6 | 4 | acceptance | a scenario walk; the server spec keeps the rule |
| `mr-boss/client/Warehouse/WarehouseProductionWalk` | 3.8 | 3 | acceptance | a scenario walk; the server spec keeps the rule |
| `mr-boss/client/Warehouse/WarehousePutawayWalk` | 3.0 | 3 | acceptance | a scenario walk; the server spec keeps the rule |
| `mr-boss/client/Warehouse/WarehouseReceivingWalk` | 9.5 | 3 | keep | the app's golden-path walk: receive a purchase order part, short receipt |
| `mr-boss/client/Warehouse/WarehouseReportsWalk` | 7.0 | 5 | acceptance | a scenario walk; the server spec keeps the rule |
| `mr-boss/client/Warehouse/WarehouseReturnsWalk` | 8.3 | 3 | acceptance | a scenario walk; the server spec keeps the rule |
| `mr-boss/client/Warehouse/WarehouseShippingWalk` | 3.7 | 3 | acceptance | a scenario walk; the server spec keeps the rule |
| `mr-boss/client/Warehouse/WarehouseTransfersWalk` | 14.9 | 4 | acceptance | a scenario walk; the server spec keeps the rule |
| `mr-marble/ClientAccess` | 0.0 | 1 | fixture | the access BrowserSmoke needs |
| `mr-marble/client/BrowserSmoke` | 8.2 | 7 | keep, trim to 3 | the one mr-marble walk: the flow data pages, PrintOrder's SVG |
| `mr-pass/Billing/PortalFixtures` | 0.4 | 1 | fixture | PortalHttp's data |
| `mr-pass/ClientFixtures` | 0.1 | 1 | fixture | the HTTP test community |
| `mr-pass/RemoteFixtures` | 0.0 | 1 | acceptance | fixture of RemoteHttp; ignored outside the remote frontend runtime |
| `mr-pass/client/ForumHttp` | 0.0 | 1 | acceptance | was marked slow (64 s); IGNORED in the measured run |
| `mr-pass/client/PortalHttp` | 4.1 | 3 | keep | the one mr-pass walk: Billing and Supplier over HTTP |
| `mr-pass/client/RemoteHttp` | 18.3 | 6 | acceptance | the remote frontend over HTTP, 18 s |
| `webroot/client/Dialog` | 1.1 | 1 | merge into moqui-html | one DOM feature |
| `webroot/client/DisabledControls` | 1.2 | 1 | merge into moqui-html | one DOM feature |
| `webroot/client/Download` | 1.0 | 1 | merge into moqui-html | one DOM feature |
| `webroot/client/HeadClient` | 1.0 | 1 | merge into moqui-html | one DOM feature |
| `webroot/client/InlineSelect` | 1.0 | 1 | merge into moqui-html | one DOM feature |
| `webroot/client/Keys` | 8.1 | 3 | keep | webroot core: html keys with real key events |
| `webroot/client/MoquiHtmlCss` | 3.9 | 7 | keep | webroot core: layout metrics |
| `webroot/client/MoquiL10n` | 0.0 | 7 | keep | webroot core |
| `webroot/client/MoquiLib` | 0.2 | 29 | keep | webroot core |
| `webroot/client/QvtKeys` | 10.3 | 3 | keep | webroot core: qvt keys, 10 s; trim the waits |
| `webroot/client/TablePopup` | 0.9 | 1 | merge into moqui-html | one DOM feature |
| `webroot/client/WebrootAssist` | 0.1 | 30 | keep | webroot core: Assist |
| `webroot/client/WebrootAssistCanvas` | 0.0 | 16 | merge into WebrootAssist | the Assist canvas |
| `webroot/client/WebrootChat` | 0.2 | 22 | keep | webroot core: the chat store |
| `webroot/client/WebrootCode` | 0.0 | 12 | merge into WebrootVue.qvt.markdown | vendored CodeMirror smoke |
| `webroot/client/WebrootPublic.qvt` | 0.0 | 4 | keep | webroot core: the public shell |
| `webroot/client/WebrootReport` | 0.2 | 46 | keep | webroot core: reports; 46 features of pure functions |
| `webroot/client/WebrootRum` | 0.0 | 4 | keep | webroot core: real-user monitoring |
| `webroot/client/WebrootSFrame` | 0.0 | 7 | keep | webroot core: SFrame on WebCrypto |
| `webroot/client/WebrootVue.qvt` | 0.1 | 25 | keep | webroot core: the qvt components |
| `webroot/client/WebrootVue.qvt.code` | 0.0 | 4 | merge into WebrootVue.qvt.markdown | vendored highlight.js smoke |
| `webroot/client/WebrootVue.qvt.coedit` | 0.3 | 9 | merge into WebrootVue.qvt.markdown | vendored Tiptap smoke |
| `webroot/client/WebrootVue.qvt.htmlview` | 0.0 | 11 | merge into WebrootVue.qvt.views | a view kind |
| `webroot/client/WebrootVue.qvt.layout` | 0.0 | 5 | keep | webroot core: split and wizard |
| `webroot/client/WebrootVue.qvt.markdown` | 0.5 | 11 | keep | webroot core: the vendored editors and viewers, one spec |
| `webroot/client/WebrootVue.qvt.records` | 0.0 | 19 | keep | webroot core: records |
| `webroot/client/WebrootVue.qvt.shell` | 0.0 | 29 | keep | webroot core: the suite shell |
| `webroot/client/WebrootVue.qvt.start` | 0.0 | 1 | merge into WebrootVue.qvt | one feature: what is set before the mount |
| `webroot/client/WebrootVue.qvt.static` | 0.1 | 26 | keep | webroot core: server-static qvt |
| `webroot/client/WebrootVue.qvt.views` | 0.0 | 21 | keep | webroot core: views |
| `webroot/client/WebrootVueCss` | 1.8 | 2 | merge into MoquiHtmlCss | computed styles, 2 features |
| `webroot/client/WebrootVueDisplay` | 1.1 | 1 | merge into WebrootVue.qvt | m-display is a qvt component |
| `webroot/client/moqui-html` | 0.0 | 16 | keep | webroot core: the html client's functions |
| **Subtotal** | **783 s** | **798** | **36 kept, 73 acceptance** | **projected 593 features, 74 s** |

## D. mr-boss app specs

The mr-boss app specs are rule tests and stay. The Projects server-side usable-when walks move to acceptance, and two
specs join the spec that owns their render. Each spec also drops the renders the [duplicate renders](#duplicate-renders)
table gives another spec, and in step 5 each app's specs are read for checks of a Mantle rule that Mantle already tests,
which go. Contacts, at 19 s, must get under the 15 s limit.

| Spec | Time | Features | Decision | Reason |
|---|--:|--:|---|---|
| `mr-boss/Accounting/AccountingBanking` | 0.8 | 4 | keep | the bank account screens |
| `mr-boss/Accounting/AccountingBudgets` | 0.5 | 4 | keep | a bill's budget override |
| `mr-boss/Accounting/AccountingCollections` | 2.2 | 6 | keep | collections, credit, recurring |
| `mr-boss/Accounting/AccountingFixedAssets` | 0.8 | 3 | keep | depreciation at close |
| `mr-boss/Accounting/AccountingIssuedFiles` | 0.4 | 3 | keep | issued evidence stays exact |
| `mr-boss/Accounting/AccountingMatching` | 0.8 | 4 | keep | BillDetail's Match tab |
| `mr-boss/Accounting/AccountingOutputs` | 0.5 | 6 | keep | printed outputs |
| `mr-boss/Accounting/AccountingPayables` | 1.7 | 8 | keep | payables and approvals |
| `mr-boss/Accounting/AccountingPaymentFiles` | 0.8 | 6 | keep | payment authority on Files |
| `mr-boss/Accounting/AccountingPaymentGateway` | 1.2 | 4 | keep | gateway actions on the payment screens |
| `mr-boss/Accounting/AccountingPaymentRefunds` | 0.8 | 4 | keep | the refund dialog |
| `mr-boss/Accounting/AccountingPaymentRuns` | 2.0 | 6 | keep | payment runs |
| `mr-boss/Accounting/AccountingPaymentTransfers` | 1.3 | 7 | keep | transfers |
| `mr-boss/Accounting/AccountingPayments` | 2.5 | 15 | keep | Accounting payments: the hub for payment logic |
| `mr-boss/Accounting/AccountingPeriodClose` | 0.8 | 4 | keep | period close |
| `mr-boss/Accounting/AccountingProductionCostClose` | 0.9 | 3 | keep | production cost close |
| `mr-boss/Accounting/AccountingReceivables` | 2.8 | 11 | keep | receivables |
| `mr-boss/Accounting/AccountingReporting` | 4.6 | 12 | keep | the report catalog per role |
| `mr-boss/Accounting/AccountingSettlement` | 1.1 | 9 | keep | settlement drafts |
| `mr-boss/Accounting/AccountingTax` | 0.5 | 6 | keep | the tax screens |
| `mr-boss/Accounting/AccountingValuation` | 0.6 | 3 | keep | landed cost on a freight bill |
| `mr-boss/Commons/Calendar` | 0.9 | 15 | keep | calendar |
| `mr-boss/Commons/CalendarSettings` | 0.7 | 8 | keep | calendar settings |
| `mr-boss/Commons/CommonsSettings` | 1.0 | 8 | keep | Home's setup card |
| `mr-boss/Commons/ContactBookSettings` | 0.7 | 5 | keep | contact book settings |
| `mr-boss/Commons/ContactImport` | 9.2 | 12 | keep | the import wizard, 9 s |
| `mr-boss/Commons/Contacts` | 19.2 | 46 | keep, under 15 s | contacts: 19 s, over the 15 s limit: one shared setup, or split by rule |
| `mr-boss/Commons/EventDetail` | 1.4 | 12 | keep | event detail |
| `mr-boss/Commons/FileShares` | 0.2 | 4 | keep | sharing |
| `mr-boss/Commons/Files` | 1.0 | 17 | keep | files and drives |
| `mr-boss/Commons/Knowledge` | 2.4 | 21 | keep | knowledge |
| `mr-boss/Commons/KnowledgeReview` | 0.9 | 5 | keep | review |
| `mr-boss/Commons/Notes` | 0.8 | 8 | keep | notes |
| `mr-boss/Commons/Notifications` | 0.6 | 10 | keep | notifications |
| `mr-boss/Commons/QuickCreate` | 0.5 | 19 | keep | QuickCreate's kinds |
| `mr-boss/Commons/Requests` | 0.5 | 10 | keep | requests |
| `mr-boss/Commons/Schedules` | 0.3 | 5 | keep | schedules |
| `mr-boss/Commons/SyncSettings` | 0.2 | 4 | keep | device sync addresses |
| `mr-boss/Commons/Tasks` | 2.1 | 15 | keep | tasks |
| `mr-boss/Commons/TodayPanel` | 0.6 | 9 | keep | Today |
| `mr-boss/Comms/CallTarget` | 0.1 | 4 | keep | the call-target provider |
| `mr-boss/Comms/Chat` | 0.7 | 19 | keep | chat |
| `mr-boss/Comms/ChatPanel` | 0.1 | 2 | keep | the tray tab |
| `mr-boss/Comms/ChatSettings` | 0.1 | 5 | keep | chat settings |
| `mr-boss/Comms/CommsHome` | 1.0 | 5 | keep | Home's counters |
| `mr-boss/Comms/CommsModeration` | 0.6 | 3 | keep | moderation |
| `mr-boss/Comms/Compose` | 0.7 | 11 | keep | the composer |
| `mr-boss/Comms/Consent` | 0.4 | 12 | keep | consent |
| `mr-boss/Comms/Inbox` | 1.1 | 12 | keep | the inbox |
| `mr-boss/Comms/InboxMailboxes` | 0.6 | 5 | keep | mailboxes |
| `mr-boss/Comms/MailAccounts` | 1.7 | 18 | keep | mail accounts |
| `mr-boss/Comms/MailSettings` | 0.9 | 10 | keep | identities, rules, out of office |
| `mr-boss/Comms/MailSignIn` | 0.3 | 7 | keep | OAuth sign-in |
| `mr-boss/Manufacturing/ManufacturingDowntime` | 1.3 | 6 | keep | downtime |
| `mr-boss/Manufacturing/ManufacturingMaterials` | 1.8 | 5 | keep | materials |
| `mr-boss/Manufacturing/ManufacturingMethods` | 1.1 | 6 | keep | methods |
| `mr-boss/Manufacturing/ManufacturingPlanning` | 0.9 | 5 | keep | supply planning |
| `mr-boss/Manufacturing/ManufacturingReports` | 2.5 | 4 | keep | reports and Kpis |
| `mr-boss/Manufacturing/ManufacturingSchedule` | 1.2 | 6 | keep | scheduling and capacity |
| `mr-boss/Manufacturing/ManufacturingShopFloor` | 2.3 | 6 | keep | shop floor and production time |
| `mr-boss/Manufacturing/ManufacturingSubcontracting` | 1.0 | 5 | keep | subcontracting |
| `mr-boss/Manufacturing/ManufacturingWorkCenters` | 0.6 | 5 | keep | work centers |
| `mr-boss/Manufacturing/ManufacturingWorkOrders` | 2.0 | 4 | keep | work orders |
| `mr-boss/Products/ProductsCatalog` | 1.6 | 12 | keep | the catalog and its capabilities |
| `mr-boss/Products/ProductsChecks` | 0.3 | 5 | keep | catalog checks |
| `mr-boss/Products/ProductsCosts` | 2.5 | 15 | keep | costs |
| `mr-boss/Products/ProductsDocuments` | 4.1 | 19 | keep | datasheets and pack labels |
| `mr-boss/Products/ProductsFinish` | 2.1 | 8 | keep | finishing a price list |
| `mr-boss/Products/ProductsImport` | 0.7 | 7 | keep | import |
| `mr-boss/Products/ProductsInventory` | 5.0 | 13 | keep | inventory |
| `mr-boss/Products/ProductsMethods` | 10.3 | 19 | keep | BOM and routing, 10 s |
| `mr-boss/Products/ProductsPriceLists` | 1.9 | 21 | keep | price lists |
| `mr-boss/Products/ProductsPricePrint` | 0.1 | 1 | merge into ProductsFinish | PriceListDetail in html again; its html-print check joins Finish's render |
| `mr-boss/Products/ProductsPricing` | 3.7 | 13 | keep | pricing |
| `mr-boss/Products/ProductsSharedContext` | 1.7 | 8 | keep | shared context |
| `mr-boss/Products/ProductsSuppliers` | 7.1 | 22 | keep | suppliers |
| `mr-boss/Products/ProductsTaxonomy` | 0.8 | 9 | keep | categories and features |
| `mr-boss/Products/ProductsTools` | 5.2 | 21 | keep | tools |
| `mr-boss/Products/ProductsVariants` | 3.9 | 8 | keep | variant rules |
| `mr-boss/Projects/Billing` | 0.8 | 5 | keep | billing |
| `mr-boss/Projects/BillingWalk` | 0.8 | 6 | acceptance | a Projects phase's usable-when walk, server side; renders the logic specs' screens again |
| `mr-boss/Projects/ClientBilling` | 1.3 | 8 | keep | billing for clients |
| `mr-boss/Projects/Code` | 1.5 | 8 | keep | code |
| `mr-boss/Projects/CodeReviewWalk` | 0.7 | 6 | acceptance | a Projects phase's usable-when walk, server side; renders the logic specs' screens again |
| `mr-boss/Projects/ColumnChooser` | 0.4 | 3 | merge into TimeEntries | the column chooser's hide condition on TimeEntries' renders (477f9e89) |
| `mr-boss/Projects/MergeRequests` | 1.2 | 7 | keep | merge requests |
| `mr-boss/Projects/MilestoneBillingWalk` | 0.9 | 8 | acceptance | a Projects phase's usable-when walk, server side; renders the logic specs' screens again |
| `mr-boss/Projects/NewProject` | 0.3 | 5 | keep | NewProject |
| `mr-boss/Projects/ProjectDetail` | 1.3 | 8 | keep | ProjectDetail tabs |
| `mr-boss/Projects/ProjectRequests` | 0.9 | 13 | keep | requests |
| `mr-boss/Projects/Projects` | 0.7 | 5 | keep | the project list and bulk |
| `mr-boss/Projects/ProjectsHome` | 0.6 | 3 | keep | Home |
| `mr-boss/Projects/ProjectsSettings` | 0.5 | 5 | keep | settings pages |
| `mr-boss/Projects/ReleaseWalk` | 1.3 | 6 | acceptance | a Projects phase's usable-when walk, server side; renders the logic specs' screens again |
| `mr-boss/Projects/Reports` | 1.4 | 7 | keep | reports and outputs |
| `mr-boss/Projects/RequestWalk` | 0.5 | 6 | acceptance | a Projects phase's usable-when walk, server side; renders the logic specs' screens again |
| `mr-boss/Projects/ResourceWalk` | 0.6 | 4 | acceptance | a Projects phase's usable-when walk, server side; renders the logic specs' screens again |
| `mr-boss/Projects/Resources` | 1.3 | 4 | keep | resources |
| `mr-boss/Projects/SprintWalk` | 1.2 | 5 | acceptance | a Projects phase's usable-when walk, server side; renders the logic specs' screens again |
| `mr-boss/Projects/TimeEntries` | 2.8 | 7 | keep | time entries |
| `mr-boss/Projects/TimeWalk` | 2.4 | 6 | acceptance | a Projects phase's usable-when walk, server side; renders the logic specs' screens again |
| `mr-boss/Projects/TimelineWalk` | 2.2 | 8 | acceptance | a Projects phase's usable-when walk, server side; renders the logic specs' screens again |
| `mr-boss/Projects/WorkItemDetail` | 1.5 | 10 | keep | WorkItemDetail |
| `mr-boss/Projects/WorkItems` | 1.7 | 9 | keep | work items and QuickCreate |
| `mr-boss/PublicShare` | 0.1 | 5 | keep | the public link page |
| `mr-boss/PublishedCalendar` | 0.2 | 4 | keep | the other token route |
| `mr-boss/Shared/Chatter` | 1.1 | 27 | keep | Chatter, used by every app |
| `mr-boss/Shared/RecordFiles` | 0.2 | 11 | keep | RecordFiles beside Chatter |
| `mr-boss/Unsubscribe` | 0.1 | 8 | keep | the public unsubscribe page |
| `mr-boss/Warehouse/WarehouseAdjustments` | 1.2 | 7 | keep | adjustments |
| `mr-boss/Warehouse/WarehouseCounts` | 1.4 | 7 | keep | counts |
| `mr-boss/Warehouse/WarehouseFacilities` | 2.8 | 32 | keep | facilities and locations |
| `mr-boss/Warehouse/WarehouseInspections` | 0.5 | 4 | keep | inspections |
| `mr-boss/Warehouse/WarehouseInventory` | 1.4 | 10 | keep | inventory and traceability |
| `mr-boss/Warehouse/WarehouseMoves` | 0.7 | 6 | keep | moves |
| `mr-boss/Warehouse/WarehousePacking` | 0.6 | 4 | keep | packing |
| `mr-boss/Warehouse/WarehousePicking` | 0.3 | 3 | keep | picking |
| `mr-boss/Warehouse/WarehousePolicies` | 1.8 | 18 | keep | policy screens |
| `mr-boss/Warehouse/WarehousePutaway` | 0.6 | 5 | keep | putaway |
| `mr-boss/Warehouse/WarehouseReceiving` | 0.9 | 5 | keep | receiving |
| `mr-boss/Warehouse/WarehouseReports` | 0.9 | 7 | keep | reports |
| `mr-boss/Warehouse/WarehouseReturns` | 0.8 | 6 | keep | returns |
| `mr-boss/Warehouse/WarehouseShipping` | 1.2 | 5 | keep | shipping |
| `mr-boss/Warehouse/WarehouseTransfers` | 0.3 | 3 | keep | transfer reads |
| **Subtotal** | **191 s** | **1066** | **113 kept, 9 acceptance** | **projected 1011 features, 180 s** |

## E. mr-mantle

The eight `test/suite.js` flows are the backbone, and every other Mantle spec checks rules of its own: they stay. The two
deletes are checked first: each of their rules must be in OrderToCashBasicFlow.

| Spec | Time | Features | Decision | Reason |
|---|--:|--:|---|---|
| `mr-mantle/RestApiTests` | 0.5 | 12 | keep | test/suite.js end-to-end flow: the backbone |
| `mr-mantle/account/AccountingBankReconciliation` | 1.1 | 11 | keep | bank statements and their reconciliation |
| `mr-mantle/account/AccountingCollections` | 1.2 | 9 | keep | collections and credit |
| `mr-mantle/account/AccountingCredit` | 1.4 | 9 | keep | customer credit and order holds |
| `mr-mantle/account/AccountingCustomerFinance` | 0.6 | 8 | keep | accounting customer finance |
| `mr-mantle/account/AccountingDelivery` | 3.1 | 12 | keep | customer invoice print, send#Invoice |
| `mr-mantle/account/AccountingInternalTransfers` | 0.9 | 4 | keep | an internal transfer is one PtInternalTransfer payment |
| `mr-mantle/account/AccountingMatching` | 3.1 | 14 | keep | bill matching |
| `mr-mantle/account/AccountingPayables` | 1.5 | 10 | keep | payables |
| `mr-mantle/account/AccountingPaymentGateway` | 0.9 | 8 | keep | reviewed gateway actions through Mantle's gateway services |
| `mr-mantle/account/AccountingPaymentLifecycle` | 2.4 | 14 | keep | payments, the area the flows touch least |
| `mr-mantle/account/AccountingPaymentNotifications` | 0.4 | 4 | keep | PaymentValidation notifications |
| `mr-mantle/account/AccountingPaymentOutput` | 2.0 | 5 | keep | issued payment documents |
| `mr-mantle/account/AccountingPaymentRefunds` | 1.6 | 4 | keep | manual refunds through Mantle's create#RefundPayment |
| `mr-mantle/account/AccountingPaymentRunControls` | 1.9 | 10 | keep | payment run and term controls |
| `mr-mantle/account/AccountingPaymentRuns` | 1.0 | 8 | keep | payment runs and bank files |
| `mr-mantle/account/AccountingPortals` | 2.9 | 17 | keep | Accounting's customer and supplier pages on CX-PORTALS |
| `mr-mantle/account/AccountingRecurringAging` | 0.8 | 7 | keep | recurring invoices |
| `mr-mantle/account/AccountingSettlement` | 2.3 | 16 | keep | settlement and matching |
| `mr-mantle/account/AccountingSettlementConcurrency` | 1.1 | 5 | keep | real independent server transactions |
| `mr-mantle/account/AccountingTermDueDays` | 0.4 | 3 | keep | an invoice's due date is a calendar day counted from its day |
| `mr-mantle/account/FinancialAccountServices` | 0.3 | 8 | keep | financial accounts |
| `mr-mantle/account/GatewayContracts` | 0.2 | 6 | keep | the common gateway contracts the provider adapters build on |
| `mr-mantle/account/InvoiceOrderApply` | 0.3 | 8 | delete | order-to-invoice is OrderToCashBasicFlow's: check each rule is there first |
| `mr-mantle/account/InvoicePaymentMisc` | 1.0 | 12 | keep | invoice and payment services the order and payroll flows do not reach |
| `mr-mantle/account/InvoicePostOrder` | 0.2 | 2 | delete | posting an order's invoice is OrderToCashBasicFlow's: check first |
| `mr-mantle/account/PaymentGateway` | 0.3 | 8 | keep | the gateway contract |
| `mr-mantle/account/PaymentMethodServices` | 0.1 | 7 | keep | payment methods |
| `mr-mantle/edi/EdiX12` | 2.1 | 17 | keep | EDI X12 |
| `mr-mantle/ext/AuthorizeNetGateway` | 0.8 | 15 | keep | the Authorize.Net card gateway |
| `mr-mantle/ext/BankFileServices` | 0.1 | 9 | keep | bank statement import |
| `mr-mantle/ext/BankOutputFiles` | 0.3 | 13 | keep | outgoing bank files |
| `mr-mantle/ext/PayTraceGateway` | 1.7 | 13 | keep | the PayTrace gateway |
| `mr-mantle/ext/ShippoGateway` | 2.5 | 12 | keep | the carrier gateway |
| `mr-mantle/facility/ContactAndFacilityServices` | 0.1 | 10 | keep | party and facility contact services, and the facility services |
| `mr-mantle/facility/WarehouseFacilities` | 0.8 | 9 | keep | facility and location setup |
| `mr-mantle/facility/WarehouseOperationPolicy` | 1.1 | 21 | keep | facilities and their policies |
| `mr-mantle/facility/WarehousePolicy` | 0.3 | 6 | keep | warehouse policy |
| `mr-mantle/facility/WarehousePutaway` | 1.7 | 8 | keep | putaway |
| `mr-mantle/forum/Mailboxes` | 4.6 | 22 | keep | forum mailboxes |
| `mr-mantle/humanres/HumanResServices` | 0.1 | 8 | keep | Employment, salary and rate services |
| `mr-mantle/humanres/PayrollAdjustments` | 0.3 | 9 | keep | payroll adjustments |
| `mr-mantle/humanres/PayrollServices` | 0.2 | 10 | keep | human resources |
| `mr-mantle/ledger/AccountingActivities` | 0.3 | 6 | keep | test/suite.js end-to-end flow: the backbone |
| `mr-mantle/ledger/AccountingApplicationRules` | 0.6 | 6 | keep | the rules every application follows |
| `mr-mantle/ledger/AccountingBookDates` | 0.7 | 8 | keep | A document's calendar date survives another actor's zone and every |
| `mr-mantle/ledger/AccountingControls` | 1.7 | 34 | keep | GL controls |
| `mr-mantle/ledger/AccountingCurrency` | 1.4 | 15 | keep | Base-currency posting |
| `mr-mantle/ledger/AccountingFiscalPeriodDays` | 0.0 | 5 | keep | the fiscal periods of a day in the user's zone at a period boundary |
| `mr-mantle/ledger/AccountingFixedAssets` | 1.4 | 11 | keep | fixed asset books |
| `mr-mantle/ledger/AccountingPaymentCorrections` | 4.1 | 9 | keep | accounting payment corrections |
| `mr-mantle/ledger/AccountingPeriodClose` | 2.3 | 8 | keep | period close |
| `mr-mantle/ledger/AccountingProjectCost` | 0.6 | 4 | keep | project cost posting |
| `mr-mantle/ledger/AccountingReceivables` | 2.1 | 22 | keep | accounting receivables |
| `mr-mantle/ledger/AccountingReports` | 0.0 | 3 | keep | accounting reports |
| `mr-mantle/ledger/AccountingSetup` | 0.9 | 7 | keep | accounting setup and activation |
| `mr-mantle/ledger/AccountingValuation` | 2.3 | 15 | keep | valuation and fixed assets |
| `mr-mantle/ledger/FinancialReports` | 1.1 | 6 | keep | financial reports |
| `mr-mantle/ledger/LedgerMaintenance` | 2.7 | 6 | keep | GL accounts, transactions, journals, period closing and reposting |
| `mr-mantle/ledger/ProductionCostClose` | 2.7 | 11 | keep | Accounting's production cost close |
| `mr-mantle/ledger/ReceiptAdjustments` | 0.1 | 3 | keep | the forward receipt repost |
| `mr-mantle/marketing/ContactListSend` | 0.4 | 14 | keep | marketing sends |
| `mr-mantle/order/AccountingReturns` | 4.2 | 15 | keep | returns and order changes |
| `mr-mantle/order/OrderBulk` | 0.3 | 4 | keep | orders stored from an external source |
| `mr-mantle/order/OrderLifecycle` | 0.5 | 9 | keep | order status beyond the flows |
| `mr-mantle/order/OrderProcureToPayBasicFlow` | 0.9 | 23 | keep | test/suite.js end-to-end flow: the backbone |
| `mr-mantle/order/OrderToCashBasicFlow` | 1.3 | 18 | keep | test/suite.js end-to-end flow: the backbone |
| `mr-mantle/order/OrderToCashTime` | 1.2 | 1 | keep | test/suite.js end-to-end flow: the backbone |
| `mr-mantle/order/ReturnToResponseBasicFlow` | 0.5 | 12 | keep | test/suite.js end-to-end flow: the backbone |
| `mr-mantle/order/Subscriptions` | 0.2 | 3 | keep | subscriptions bought through order items |
| `mr-mantle/other/AccountingBudgets` | 2.6 | 11 | keep | budgets |
| `mr-mantle/other/AccountingTax` | 1.3 | 13 | keep | tax |
| `mr-mantle/other/BudgetServices` | 0.2 | 7 | keep | budgets |
| `mr-mantle/other/TaxStatementServices` | 0.2 | 5 | keep | 1099 tax statements and the tax info lookups |
| `mr-mantle/party/ChatRecords` | 4.0 | 9 | keep | chat on the Mantle side of IronStone M10 |
| `mr-mantle/party/Consent` | 0.7 | 17 | keep | consent and unsubscribe |
| `mr-mantle/party/ContactAddresses` | 0.5 | 4 | keep | mantle.party.ContactBookServices.filter#ContactAddresses |
| `mr-mantle/party/ContactBooks` | 2.7 | 20 | keep | contact books and personal contacts |
| `mr-mantle/party/Contacts` | 1.3 | 12 | keep | parties and contacts |
| `mr-mantle/party/ContentRecords` | 0.0 | 3 | keep | content records of payments, agreements and parties |
| `mr-mantle/party/CrmMiscServices` | 0.3 | 7 | keep | communication events, requests, sales accounts and contacts |
| `mr-mantle/party/InboxThreads` | 3.9 | 13 | keep | the Inbox over both stores and mail on records |
| `mr-mantle/party/InternalLabels` | 0.2 | 1 | keep | labels on internal messages |
| `mr-mantle/party/InternalMessages` | 2.7 | 15 | keep | messages, inbox, chat records |
| `mr-mantle/party/PartyAccounts` | 0.1 | 5 | keep | party search, user account maintenance and notice counts |
| `mr-mantle/party/PartyServices` | 0.1 | 8 | keep | party services the flows do not reach |
| `mr-mantle/party/PartyTimePeriods` | 0.1 | 6 | keep | time period maintenance |
| `mr-mantle/party/RecordLinks` | 0.4 | 19 | keep | record links, the record timeline, followers and mentions |
| `mr-mantle/party/RecordNotes` | 0.2 | 7 | keep | notes on a record changed and deleted |
| `mr-mantle/party/RecordVisibility` | 1.9 | 11 | keep | which records a user may see through the record link services |
| `mr-mantle/pass/PassAuthority` | 0.1 | 5 | keep | the PASS authority mr-pass builds on |
| `mr-mantle/product/AssetReservationMisc` | 0.4 | 11 | keep | Assets, reservations, issuance and physical inventory |
| `mr-mantle/product/AssetReservationMultipleThreads` | 0.9 | 1 | keep | test/suite.js end-to-end flow: the backbone |
| `mr-mantle/product/ProductCatalogChecks` | 0.6 | 8 | keep | product catalog checks |
| `mr-mantle/product/ProductCatalogCsv` | 0.0 | 7 | keep | product catalog CSV |
| `mr-mantle/product/ProductCatalogImport` | 1.2 | 10 | keep | product catalog import |
| `mr-mantle/product/ProductCatalogServices` | 0.2 | 10 | keep | the catalog |
| `mr-mantle/product/ProductCatalogSettings` | 0.3 | 4 | keep | the organization catalog and variant policies |
| `mr-mantle/product/ProductContents` | 0.6 | 10 | keep | product contents |
| `mr-mantle/product/ProductEffectivePrices` | 0.1 | 4 | keep | product effective prices |
| `mr-mantle/product/ProductFeaturePolicy` | 0.0 | 2 | keep | product feature policy |
| `mr-mantle/product/ProductLabels` | 0.8 | 9 | keep | product labels |
| `mr-mantle/product/ProductLifecycle` | 0.1 | 3 | keep | product lifecycle |
| `mr-mantle/product/ProductMethods` | 2.2 | 11 | keep | product methods |
| `mr-mantle/product/ProductPackaging` | 0.6 | 9 | keep | product packaging |
| `mr-mantle/product/ProductPriceLists` | 0.5 | 10 | keep | prices |
| `mr-mantle/product/ProductPricePolicy` | 0.2 | 5 | keep | product price policy |
| `mr-mantle/product/ProductStandardCosts` | 4.2 | 25 | keep | standard costs and methods |
| `mr-mantle/product/ProductStockTotals` | 0.0 | 1 | keep | mantle.product.AssetServices.get#ProductStockTotals |
| `mr-mantle/product/ProductVariants` | 0.2 | 6 | keep | product variants |
| `mr-mantle/product/Promotions` | 0.1 | 5 | keep | promotion codes and the promotion services |
| `mr-mantle/product/ReportLookups` | 0.1 | 5 | keep | lookup and report services |
| `mr-mantle/product/SupplyPlanning` | 1.5 | 11 | keep | the shared supply plan |
| `mr-mantle/product/WarehouseCounts` | 2.4 | 7 | keep | stock: counts, moves, quality, expiry, traceability |
| `mr-mantle/product/WarehouseExpiry` | 0.3 | 6 | keep | the expiry gate |
| `mr-mantle/product/WarehouseMoves` | 1.5 | 10 | keep | the Warehouse posting guard on moves |
| `mr-mantle/product/WarehouseQuality` | 2.6 | 7 | keep | shared quality inspection |
| `mr-mantle/product/WarehouseReports` | 1.1 | 9 | keep | warehouse reports |
| `mr-mantle/product/WarehouseStockChecks` | 0.2 | 5 | keep | Mantle behaviors the Warehouse phases build on |
| `mr-mantle/product/WarehouseTraceability` | 1.0 | 11 | keep | stock ledger, lineage and stock records |
| `mr-mantle/sales/AccountingStatements` | 1.3 | 10 | keep | statements and sales reports |
| `mr-mantle/sales/SalesReports` | 0.1 | 4 | keep | sales summaries over the search index |
| `mr-mantle/search/MantleSearch` | 0.4 | 8 | keep | Mantle's search callers on the search facade |
| `mr-mantle/search/UniversalSearch` | 0.3 | 7 | keep | search |
| `mr-mantle/shipment/CarrierServices` | 0.1 | 8 | keep | carrier services |
| `mr-mantle/shipment/ShipmentPick` | 0.2 | 6 | keep | picking into containers |
| `mr-mantle/shipment/ShipmentReturnMisc` | 0.6 | 8 | keep | shipment packages, picking, info and return services the flows do not |
| `mr-mantle/shipment/WarehouseAdjustments` | 1.8 | 8 | keep | warehouse adjustments, scrap, lot corrections and repack |
| `mr-mantle/shipment/WarehousePacking` | 3.5 | 9 | keep | outbound |
| `mr-mantle/shipment/WarehousePicking` | 1.8 | 8 | keep | picking |
| `mr-mantle/shipment/WarehouseReceiving` | 3.6 | 12 | keep | inbound |
| `mr-mantle/shipment/WarehouseReturns` | 1.9 | 5 | keep | returns in the warehouse |
| `mr-mantle/shipment/WarehouseShipping` | 2.9 | 10 | keep | Shipping, labels and printers |
| `mr-mantle/shipment/WarehouseTransfers` | 2.6 | 8 | keep | Transfers, stock requests and the subcontract dispatch |
| `mr-mantle/work/CalendarChanges` | 2.1 | 10 | keep | the calendar gaps of IronStone M7 on the Mantle side |
| `mr-mantle/work/CalendarServices` | 2.1 | 20 | keep | calendar |
| `mr-mantle/work/CommonsToday` | 1.6 | 14 | keep | the Today panel and the header badge counts |
| `mr-mantle/work/Dav` | 0.7 | 8 | keep | CalDAV and CardDAV over Mantle |
| `mr-mantle/work/ManufacturingServices` | 0.3 | 7 | keep | production runs |
| `mr-mantle/work/MethodTemplates` | 0.7 | 4 | keep | manufacturing P2's method services |
| `mr-mantle/work/ProductionChanges` | 4.8 | 10 | keep | controlled changes of a production run |
| `mr-mantle/work/ProductionCosts` | 1.1 | 4 | keep | production cost evidence |
| `mr-mantle/work/ProductionDowntime` | 1.4 | 7 | keep | production downtime and maintenance |
| `mr-mantle/work/ProductionMaterials` | 2.2 | 5 | keep | the materials of production runs |
| `mr-mantle/work/ProductionOrders` | 3.9 | 11 | keep | production orders |
| `mr-mantle/work/ProductionPolicy` | 2.0 | 7 | keep | the production policy and reason codes |
| `mr-mantle/work/ProductionQuality` | 3.7 | 8 | keep | production quality gates |
| `mr-mantle/work/ProductionQuantities` | 4.8 | 9 | keep | production quantities, by-products and loss |
| `mr-mantle/work/ProductionReports` | 1.3 | 6 | keep | production reports |
| `mr-mantle/work/ProductionSchedule` | 1.2 | 7 | keep | the production schedule and capacity |
| `mr-mantle/work/ProductionShopFloor` | 2.9 | 9 | keep | shop floor |
| `mr-mantle/work/ProductionSubcontract` | 1.8 | 8 | keep | outside processing |
| `mr-mantle/work/ShipmentLoads` | 0.5 | 9 | keep | shipment loads |
| `mr-mantle/work/SupportedAssembly` | 0.6 | 4 | keep | the reversal of a supported pick assembly |
| `mr-mantle/work/TimeImports` | 0.1 | 7 | keep | time entry imports |
| `mr-mantle/work/WarehouseProduction` | 2.4 | 8 | keep | production runs and the production stock guards |
| `mr-mantle/work/WorkCenters` | 1.6 | 6 | keep | work centers |
| `mr-mantle/work/WorkGit` | 0.2 | 6 | keep | git repositories of projects, and their commits |
| `mr-mantle/work/WorkGitReview` | 0.5 | 10 | keep | merge requests, reviews and repository rights on projects |
| `mr-mantle/work/WorkMyTasks` | 0.1 | 1 | keep | the current user's open tasks |
| `mr-mantle/work/WorkPlanToCashBasicFlow` | 0.8 | 11 | keep | test/suite.js end-to-end flow: the backbone |
| `mr-mantle/work/WorkProjectBilling` | 0.6 | 7 | keep | project billing |
| `mr-mantle/work/WorkProjectClients` | 1.3 | 12 | keep | client access in the project services |
| `mr-mantle/work/WorkProjectFoundation` | 0.4 | 5 | keep | the foundation of IronStone Projects in mantle.work.ProjectServices |
| `mr-mantle/work/WorkProjectHub` | 0.5 | 7 | keep | the project hub of IronStone Projects in mantle.work.ProjectServices |
| `mr-mantle/work/WorkProjectItems` | 0.6 | 10 | keep | projects and work items |
| `mr-mantle/work/WorkProjectMilestones` | 0.6 | 10 | keep | IronStone Projects billing, second part |
| `mr-mantle/work/WorkProjectPlan` | 0.4 | 10 | keep | IronStone Projects planning on the timeline and work item import |
| `mr-mantle/work/WorkProjectReleases` | 0.3 | 7 | keep | IronStone Projects milestones, releases and documentation |
| `mr-mantle/work/WorkProjectReports` | 0.7 | 8 | keep | IronStone Projects reports in mantle.work.ProjectReportServices |
| `mr-mantle/work/WorkProjectRequests` | 0.6 | 13 | keep | the requests of IronStone Projects in mantle.request.RequestServices |
| `mr-mantle/work/WorkProjectServices` | 0.2 | 9 | keep | work effort, project, task, time-entry and event services |
| `mr-mantle/work/WorkProjectSprints` | 0.3 | 6 | keep | IronStone Projects sprints and the backlog |
| `mr-mantle/work/WorkResources` | 1.0 | 6 | keep | IronStone Projects resources in mantle.work.ResourceServices |
| `mr-mantle/work/WorkTimeEntries` | 1.5 | 8 | keep | time |
| `mr-mantle/work/WorkTimesheets` | 1.9 | 8 | keep | IronStone Projects time in mantle.work.TimesheetServices |
| **Subtotal** | **210 s** | **1574** | **174 kept, 0 acceptance** | **projected 1564 features, 209 s** |

## F. Small components

The small components' specs are rule specs and stay. BasicComms drops its every-screen qvt render, ArtifactIndex builds
its index once, and ScreenReport and Scale move to acceptance.

| Spec | Time | Features | Decision | Reason |
|---|--:|--:|---|---|
| `basic-comms/BasicComms` | 3.4 | 8 | keep | BasicComms: drops the every-screen qvt render (a sweep), keeps its logic |
| `basic-comms/BasicCommsBridges` | 0.2 | 1 | keep | bridges |
| `basic-comms/BasicCommsCalls` | 0.4 | 8 | keep | calls |
| `basic-comms/BasicCommsChat` | 0.2 | 3 | keep | chat |
| `basic-comms/BasicCommsChatChanges` | — | — | keep | chat message changes in a per-room feed (MD5 stage 1) |
| `basic-comms/BasicCommsComposeRef` | 0.2 | 7 | keep | compose retry |
| `basic-comms/BasicCommsEncrypted` | 0.1 | 7 | keep | encrypted rooms |
| `basic-comms/BasicCommsFiles` | 0.1 | 5 | keep | the drive picker |
| `basic-comms/BasicCommsInbox` | 0.1 | 4 | keep | inbox |
| `basic-comms/BasicCommsSync` | — | — | keep | the desktop client's sync on the changes feeds (MD5 stage 3) |
| `basic-comms/BasicCommsTexts` | 0.1 | 2 | keep | texts |
| `dev-aide/ArtifactDocs` | 0.6 | 7 | keep | Autodoc from the index |
| `dev-aide/ArtifactIndex` | 9.2 | 7 | keep | the artifact index, 9 s: build it once for all features |
| `dev-aide/EntityModeler` | 0.1 | 7 | keep | the entity modeler |
| `dev-aide/ServiceWriter` | 0.7 | 7 | keep | the service writer |
| `example/Chat` | 0.1 | 4 | keep | chat SQL on the real database |
| `example/Example` | 0.0 | 7 | keep | the Rust service and tool, the example's point |
| `example/Files` | 0.1 | 4 | keep | drives and files |
| `example/GitHosting` | 0.1 | 3 | keep | git hosting |
| `example/MailClient` | 0.2 | 9 | keep | mail client SQL |
| `example/Wiki` | 0.2 | 5 | keep | notes spaces |
| `framework/CallCaptions` | 0.0 | 5 | keep | captions |
| `framework/CallEncryption` | 0.2 | 7 | keep | encrypted calls |
| `framework/CallHandling` | 0.1 | 9 | keep | call handling |
| `framework/CallQueue` | 0.1 | 5 | keep | queues |
| `framework/CallRecording` | 0.2 | 11 | keep | recording |
| `framework/Calls` | 0.2 | 18 | keep | calls: the core of the call services |
| `framework/ChatKeyBackup` | 0.2 | 6 | keep | key backup |
| `framework/ChatKeys` | 1.3 | 11 | keep | chat devices and keys |
| `framework/DatePeriod` | 1.0 | 6 | keep | date periods |
| `framework/EdgeVoice` | 0.1 | 6 | keep | SIP on an edge node |
| `framework/FieldTitle` | 0.0 | 6 | keep | field titles |
| `framework/Ivr` | 0.1 | 4 | keep | IVR |
| `framework/PushDevice` | — | — | keep | push devices: FCM answers, outages, ring ends (MD6) |
| `framework/RecentAuth` | 0.1 | 2 | keep | recent sign-in |
| `framework/ResourceSearchExclusions` | 0.1 | 4 | keep | resource search exclusions |
| `framework/RingGroup` | 0.1 | 8 | keep | ring groups |
| `framework/ScreenHtml` | 0.2 | 13 | keep | html screen runtime |
| `framework/ScreenQvt` | 0.2 | 16 | keep | qvt screen runtime |
| `framework/SipDevice` | 0.1 | 10 | keep | SIP devices |
| `framework/Transfer` | 0.2 | 9 | keep | transfer |
| `framework/Units` | 0.3 | 6 | keep | unit formatting |
| `framework/Voice` | 0.2 | 13 | keep | voice |
| `framework/Voicemail` | 0.5 | 11 | keep | voicemail |
| `framework/WebPush` | 0.1 | 6 | keep | user push subscriptions |
| `framework/XmppServices` | 0.0 | 7 | keep | XMPP calls |
| `mr-marble/OrderRoleFilters` | 0.0 | 20 | keep | FindOrder's role filters, no database |
| `mr-marble/ProjectSummarySource` | 0.0 | 1 | keep | one source check |
| `mr-marble/ScreenReport` | 0.0 | 1 | acceptance | not a check: writes the screen report on request |
| `mr-pass/Account/PublicIdentity` | 0.1 | 6 | keep | public identity |
| `mr-pass/Account/SsoHandoff` | 0.0 | 2 | keep | SSO handoff |
| `mr-pass/Billing/Portals` | 0.3 | 3 | keep | the portals through the PASS facade |
| `mr-pass/Forum/Conversations` | 1.1 | 11 | keep | private conversations |
| `mr-pass/Forum/Forum` | 0.5 | 12 | keep | the forum authority contract |
| `mr-pass/Forum/ForumAccess` | 0.9 | 15 | keep | access |
| `mr-pass/Forum/Import` | 0.6 | 7 | keep | Discourse import |
| `mr-pass/Forum/InboundMail` | 0.4 | 11 | keep | email participation |
| `mr-pass/Forum/Integrations` | 1.1 | 6 | keep | integrations |
| `mr-pass/Forum/Media` | 0.2 | 5 | keep | media |
| `mr-pass/Forum/Moderation` | 1.2 | 12 | keep | moderation |
| `mr-pass/Forum/Operations` | 1.0 | 9 | keep | retention runs |
| `mr-pass/Forum/Outbox` | 1.3 | 13 | keep | outbox |
| `mr-pass/Forum/Participation` | 0.9 | 11 | keep | participation |
| `mr-pass/Forum/Scale` | 0.0 | 1 | acceptance | opt-in large fixture |
| `mr-pass/Members/Entitlements` | 1.1 | 9 | keep | entitlements |
| `mr-pass/Transport` | 0.0 | 2 | keep | the authority's transport |
| `tools/ConfReload` | 0.2 | 6 | keep | conf reload and components |
| `tools/DataView` | 1.4 | 1 | keep | Data View |
| `tools/MonitorSettings` | 0.1 | 2 | keep | monitor settings |
| **Subtotal** | **34 s** | **470** | **64 kept, 2 acceptance** | **projected 468 features, 34 s** |

## Duplicate renders

Screens that more than one regression spec renders in the same mode after the cut, from a static scan of `page()`,
`render()`, `tui()` and `qvt()` calls with a literal path (paths relative to each app's root screen). A call without a
mode takes its helper's default, read from each spec: qvt for the Accounting, Manufacturing and Warehouse SpecLibs, the
Projects and Commons specs' own helpers and the Comms specs; html for ProductsFinish and ProductsPricePrint; Forum's
helpers set none ("default"). Paths built in loops are not seen; those specs are in group A and go. The keeper owns the
screen's logic: never the access spec when a logic spec also renders it, then the spec named for a segment of the
screen's path. The others drop their render of the screen and carry any check of it over to the keeper.

| App | Screen | Mode | Rendered by (regression specs after the cut) | Render that keeps the checks |
|---|---|---|---|---|
| Accounting | `Approvals` | qvt | AccountingAccess, AccountingPayables | AccountingPayables |
| Accounting | `Banking/BankAccountDetail` | qvt | AccountingAccess, AccountingBanking | AccountingBanking |
| Accounting | `Banking/BankAccounts` | qvt | AccountingAccess, AccountingBanking | AccountingBanking |
| Accounting | `Payables/BillDetail` | qvt | AccountingBudgets, AccountingPayables | AccountingPayables |
| Accounting | `Payables/BillDetail/Match` | qvt | AccountingAccess, AccountingMatching | AccountingMatching |
| Accounting | `Payments/PaymentDetail` | qvt | AccountingPaymentGateway, AccountingPayments | AccountingPayments |
| Accounting | `Payments/PaymentDetail/Review` | qvt | AccountingPaymentGateway, AccountingPaymentRefunds | AccountingPaymentGateway |
| Accounting | `PeriodClose/PeriodCloseDetail` | html | AccountingOutputs, AccountingPeriodClose | AccountingPeriodClose |
| Accounting | `Receivables/Collections` | qvt | AccountingAccess, AccountingCollections | AccountingCollections |
| Accounting | `Receivables/InvoiceDetail` | html | AccountingAccess, AccountingReceivables | AccountingReceivables |
| Accounting | `Records/OrderDetail` | qvt | AccountingCollections, AccountingPayables | AccountingCollections |
| Commons | `Files/DriveSettings` | qvt | Files, SyncSettings | Files |
| Commons | `Files/download` | qvt | FileShares, Files | Files |
| Commons | `Home` | qvt | CommonsSettings, Files, Notes | CommonsSettings |
| Commons | `Settings/Calendars` | qvt | CalendarSettings, SyncSettings | CalendarSettings |
| Commons | `Settings/ContactBooks` | qvt | ContactBookSettings, SyncSettings | ContactBookSettings |
| Commons | `Settings/Preferences` | qvt | CalendarSettings, CommonsSettings | CommonsSettings |
| Comms | `Account` | qvt | MailAccounts, MailSignIn | MailAccounts |
| Comms | `Add/Connect` | qvt | MailAccounts, MailSignIn | MailAccounts |
| Comms | `Chatter/timeline` | qvt | Chat, Compose | Chat |
| Comms | `List` | qvt | MailAccounts, MailSignIn | MailAccounts |
| Manufacturing | `Approvals` | html | ManufacturingAccess, ManufacturingMethods | ManufacturingMethods |
| Manufacturing | `Home` | html | ManufacturingMaterials, ManufacturingPlanning, ManufacturingReports | ManufacturingReports |
| Manufacturing | `Planning/Capacity` | html | ManufacturingAccess, ManufacturingReports, ManufacturingSchedule | ManufacturingSchedule |
| Manufacturing | `Planning/ProductionScheduleTui` | tui | ManufacturingAccess, ManufacturingSchedule | ManufacturingSchedule |
| Manufacturing | `Planning/SupplyPlanning` | qvt | ManufacturingPlanning, ManufacturingSubcontracting | ManufacturingPlanning |
| Manufacturing | `Planning/SupplyPlanning` | html | ManufacturingAccess, ManufacturingPlanning | ManufacturingPlanning |
| Manufacturing | `Planning/SupplyPlanning/convertSuggestions` | qvt | ManufacturingPlanning, ManufacturingSubcontracting | ManufacturingPlanning |
| Manufacturing | `ShopFloor/OperationExecution` | html | ManufacturingAccess, ManufacturingShopFloor | ManufacturingShopFloor |
| Manufacturing | `ShopFloor/ProductionTime` | html | ManufacturingAccess, ManufacturingShopFloor | ManufacturingShopFloor |
| Manufacturing | `ShopFloor/Station` | tui | ManufacturingAccess, ManufacturingShopFloor | ManufacturingShopFloor |
| Manufacturing | `WorkCenters/EquipmentDetail` | qvt | ManufacturingDowntime, ManufacturingWorkCenters | ManufacturingWorkCenters |
| Manufacturing | `WorkCenters/FacilityDetail/WorkCenter` | qvt | ManufacturingSchedule, ManufacturingWorkCenters | ManufacturingWorkCenters |
| Manufacturing | `WorkOrders/Find` | html | ManufacturingAccess, ManufacturingWorkOrders | ManufacturingWorkOrders |
| Manufacturing | `WorkOrders/WorkOrderDetail` | html | ManufacturingAccess, ManufacturingWorkOrders | ManufacturingWorkOrders |
| Projects | `Billing/InvoiceDetail` | qvt | Billing, ClientBilling, ClientIsolation | Billing |
| Projects | `Billing/Invoices` | qvt | Billing, ClientBilling, ClientIsolation | Billing |
| Projects | `Billing/Unbilled` | qvt | Billing, ClientBilling | Billing |
| Projects | `Code/CommitDetail` | qvt | ClientIsolation, Code | Code |
| Projects | `Code/Compare` | qvt | ClientIsolation, Code | Code |
| Projects | `Code/MergeRequestDetail` | qvt | ClientIsolation, MergeRequests | MergeRequests |
| Projects | `Code/MergeRequestDetail/Changes` | qvt | ClientIsolation, MergeRequests | MergeRequests |
| Projects | `Code/MergeRequestDetail/ChangesTui` | qvt | ClientIsolation, MergeRequests | MergeRequests |
| Projects | `Code/MergeRequestDetail/Commits` | qvt | ClientIsolation, MergeRequests | MergeRequests |
| Projects | `Code/MergeRequestDetail/Overview` | qvt | ClientIsolation, MergeRequests | MergeRequests |
| Projects | `Code/MergeRequests` | qvt | ClientIsolation, MergeRequests | MergeRequests |
| Projects | `Code/Repositories` | qvt | ClientIsolation, Code | Code |
| Projects | `Code/RepositoryDetail` | qvt | ClientIsolation, Code | Code |
| Projects | `Code/RepositoryDetail/MergeRequests` | qvt | ClientIsolation, MergeRequests | MergeRequests |
| Projects | `Home` | qvt | Billing, ClientIsolation, MergeRequests, Reports, Resources | Billing |
| Projects | `Projects/ProjectDetail` | qvt | ClientIsolation, ProjectDetail | ProjectDetail |
| Projects | `Projects/ProjectDetail/Code` | qvt | ClientIsolation, Code | Code |
| Projects | `Projects/ProjectDetail/Overview` | qvt | ClientIsolation, ProjectDetail | ProjectDetail |
| Projects | `Projects/ProjectDetail/Team` | qvt | ProjectDetail, Resources | ProjectDetail |
| Projects | `Projects/ProjectDetail/Time` | qvt | ClientBilling, TimeEntries | TimeEntries |
| Projects | `Time/TimeEntries` | qvt | Reports, TimeEntries | TimeEntries |
| Projects | `WorkItems/WorkItemDetail` | qvt | ClientIsolation, MergeRequests, TimeEntries, WorkItemDetail | WorkItemDetail |
| Projects | `apps/Commons/Tasks/TaskDetail` | qvt | MergeRequests, WorkItemDetail | WorkItemDetail |
| Warehouse | `Counts/Counts` | html | WarehouseAccess, WarehouseCounts | WarehouseCounts |
| Warehouse | `Handheld/Home` | html | WarehouseAccess, WarehouseMoves | WarehouseMoves |
| Warehouse | `Handheld/Move` | html | WarehouseAccess, WarehouseMoves | WarehouseMoves |
| Warehouse | `Home` | html | WarehouseMoves, WarehouseReports | WarehouseReports |
| Warehouse | `Inspection/Inspections` | html | WarehouseAccess, WarehouseInspections | WarehouseInspections |
| Warehouse | `Inventory/Adjustments` | html | WarehouseAccess, WarehouseAdjustments | WarehouseAdjustments |
| Warehouse | `Inventory/AssetDetail` | html | WarehouseAdjustments, WarehouseInventory | WarehouseInventory |
| Warehouse | `Inventory/Assets` | html | WarehouseAccess, WarehouseInventory | WarehouseInventory |
| Warehouse | `Receiving/Putaway` | html | WarehouseAccess, WarehousePutaway | WarehousePutaway |
| Warehouse | `Receiving/ReceiptDetail` | html | WarehouseAccess, WarehousePicking, WarehouseReceiving | WarehouseReceiving |
| Warehouse | `Receiving/Receipts` | html | WarehouseAccess, WarehouseReceiving | WarehouseReceiving |
| Warehouse | `Returns/Returns` | html | WarehouseAccess, WarehouseReturns | WarehouseReturns |
| Warehouse | `Shipping/Pack` | html | WarehouseAccess, WarehousePacking | WarehousePacking |
| Warehouse | `Shipping/ShipmentDetail` | html | WarehousePacking, WarehousePicking | WarehousePacking |
| Warehouse | `Transfers/Move` | html | WarehouseAccess, WarehouseMoves | WarehouseMoves |
| Warehouse | `Transfers/Repack` | html | WarehouseAccess, WarehouseAdjustments | WarehouseAdjustments |
| Forum | `Account/Notifications` | default | Moderation, Outbox | Moderation |
| Forum | `Forum/Topics/TopicDetail` | default | Moderation, Participation | Participation |

## Method

The group tables are the inventory: each spec's time and features come from the measured log, summed from its
`Spec > feature  PASSED (1.23s)` lines and mapped to its file (`find runtime -name '*.spec.js'`), using the stage and
the feature titles where two files share a name. The other tables are generated from the group tables by a short script:
it applies each row's decision and the projection formula above, resolves each render call's mode from the spec's
helper, and checks that every `merge into` names a kept spec of the same component and that every spec file under
`runtime` has a row. Regenerate them after any change to a decision.
