# Test reduction

A proposal in two parts. First, split the tests into two suites with different purposes: a small **regression suite**
that developers and agents run locally before every commit, and a separate **acceptance suite** of complete scenario
and system checks, run before a deploy or for general system validation, never as part of normal development. Second,
cut the regression suite by about 80%, moving the scenario checks worth keeping to the acceptance suite and deleting the
rest, so that `./moq test-all` runs its tests in under 10 minutes and walk-everything and every-mode render tests are
gone from it for good. Nothing is moved or deleted yet: this document is for review, and the changes follow area by
area once it is approved.

**The 10 minutes is a goal for `test-all` without the Rust build time, not a constraint.** Application work
runs only the app tests ([below](#framework-tests-and-app-tests)), aiming at about 3½ minutes. On this machine a cold release build
takes about 5 minutes, and a debug build for `cargo test` about as long again, so with the builds no test run could fit in
10 minutes. Builds are measured apart: a test run should need a rebuild only after a change to the Rust code or to what it
embeds, and more test runs over time should reuse the binaries already built (`--nobuild`, sccache) rather than rebuild.

## Why

The last test-all ran its tests for about 37 minutes, besides the builds: 4,919 server features in 881 s (0.18 s each)
and 425 browser features in 814 s (about 2 s each). A page renders in milliseconds; the time is the multiplication.
The suite grew with screens × users × render modes × authorization on and off × apps, not with the rules it checks, so
every new screen, group or app made every run longer, and with all the apps built it heads for hours. The rules made it
so: "every new screen in AppStructure and TuiRender", "a walk for each scenario", phase Done rules needing access-spec
features and phone-width walks per phase, and agents followed them faithfully (the Manufacturing build alone added about
20 specs, 16 walks and an access-spec section per phase). An oversized regression suite is a performance bug of the
system as a whole, like reading the whole conf tree to get one value, and the `slow` flag only hid the worst of it.

## Two suites

| | Regression suite | Acceptance suite |
|---|---|---|
| Purpose | Catch a regression while developing: does what I changed break a rule? | Validate the system: complete scenarios, every route per group, every screen, browsers, isolation, before a deploy or on a schedule |
| When | While working (the specs of what you touch) and before every commit | Before a deploy, nightly or on a schedule (CI or a spare machine), and when asked |
| Who runs it | Every developer and agent, locally | CI or a release step; an agent only when asked (below) |
| Where | `test/*.spec.js`, `test/client/*.spec.js` (as now) | `acceptance/*.spec.js` and `acceptance/client/*.spec.js` in each component, a separate tree |
| Command | `moqui test`, `moqui test-client`, `./moq test-apps`, `./moq test-framework`, `./moq test-all` | `./moq accept [component...]` (`moqui test --acceptance`, `moqui test-client --acceptance`); `./moq accept-all` |
| Discovery | Never sees `acceptance/` | Only `acceptance/` |
| Size | A budget per component, enforced (below) | No budget; as complete as it needs to be |
| Failure | Blocks the commit | Reported; fixed or the test deleted, never blocks a commit |

- **Separate by location, not by a flag.** A test is in one suite or the other by the directory it is in. No option of
  the regression commands pulls in acceptance tests, so they cannot leak into normal runs the way `slow` specs did when
  named or passed `--slow`.
- **What goes where.** A regression test checks one rule or a fixed bug: a service's result and refusals, a posting, a
  state change, the one render of a screen with logic of its own, one golden-path walk per app. An acceptance test checks
  a scenario or the system: a plan's "usable when" walk, an access or isolation walk over every route and group, a sweep
  of every screen in a mode, a browser walk of a flow, a load or media test. The acceptance suite may be large; its tests
  can be deleted freely, since nothing depends on them.
- **Shared data and fixtures.** Each suite loads its own data once per run; acceptance fixtures live under
  `acceptance/` and never in `test/`, so a regression run loads none of them.

### When an agent runs the acceptance suite

Only:

1. when the user asks for it (a component's acceptance tests, one spec, or all), or
2. when the agent adds or changes an acceptance test: then it runs that test, and the acceptance suite of the component
   it is in, once the test is written.

Never because code they use changed, never to check its own work, and never as part of "done" for a phase. A plan or
brief that wants scenario checks names the acceptance tests to write; running them is the user's call.

### The regression budget

- Each component declares its regression budget in seconds of feature time, for example in its `component.xml` or a
  `test/budget` file: mr-boss 60 s, mr-mantle 45 s, mr-pass 10 s, mr-marble 5 s, the framework's components together
  90 s (to be set from the cut below).
- `moqui test` and `moqui test-client` fail a run when a component's feature time goes over its budget, or when one spec
  takes over 15 s. That replaces today's end-of-run warning for specs over 30 s: a warning did not stop the growth.
- A new app adds its own budget, so test-all grows by a known amount per app rather than combinatorially.
- A budget is raised only with the user's agreement, like any other performance regression.

### The slow flag afterwards

Once the acceptance suite exists, `slow: true` is rare: a long test belongs in the acceptance suite or is deleted. The
slow-test instructions shrink to one line (a regression test over 15 s fails the run; move it to acceptance or delete
it), and the specs marked slow now (AppStructure, TuiRender, AccountingAccessWalk, the ServerStatic family, the three
call walks, mr-pass ForumHttp and Scale) move to the acceptance suite or are deleted per the tables below.

### Cheaper machinery

Besides the content, the run itself wastes time; these make the small suite fast in practice:

- **One server for all served client specs** of a run instead of one per component, each loading its own database
  (about 200 s of the measured `--serve`).
- **One data load per run**, reused through a snapshot.
- **Fixtures once per component**, not per spec.
- **Builds apart:** a test command rebuilds only after a change to the Rust code or what it embeds; otherwise it reuses
  the binary (`--nobuild` as the default when the binary is newer than the sources, sccache).
- **Later:** specs of independent data in parallel workers, each on its own database copy. A framework investment, after
  the content is cut, not instead of it.

The rules it applies are already in [Build-Test](../docs/Build-Test.md#rendering-screens-in-tests) and `AGENTS.md`:
no test walks every screen, a screen is rendered at most once per render mode across the suite, and a test over 30
seconds is marked slow and kept out of normal runs.

## Instructions to update

The tests grew because the instructions asked for them, so the cut does not hold unless the instructions change with it.
The inventory below is every place that says which tests to write or run, as of Nov 4, 2026 (paths in mr-moqui unless
a component is named). Rules marked **growth** made the suite grow directly.

### The new test-writing rules (the text the edits put in place)

- **Two suites.** The regression suite (`test/`) is for development time: small, run while working and before every
  commit. The acceptance suite (`acceptance/`) is for pre-deploy and system validation: complete scenarios, run by CI or
  when asked. Say which suite a new test goes in, and why.
- **Regression tests check rules, not pages.** Write one for a business rule you add or change (its result and its
  refusals) and for a bug you fix (the test that would have caught it). Access: one holder and one non-holder on one verb
  of the rule, and another organization's record refused; not every route for every group. A screen is rendered in a
  regression test only when it has logic of its own, once per mode, with all of its checks on that render, and in a
  second mode only where it has its own qvt or tui template. No `where:` doubling of authorization on and off, no users ×
  modes matrices. Mantle rules are tested in Mantle, not again through an app.
- **Walks.** The regression suite has one golden-path client walk per app, kept current. Every other browser walk (a
  plan's scenario, a phone-width check, an access or isolation walk) is an acceptance test.
- **Budget.** A regression test runs in well under a second as a rule; one over 15 s fails the run. A component over its
  budget fails the run; cut or move tests rather than raise the budget.
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
| `AGENTS.md` slow-tests item (line 5) | Never run a slow test without permission; over 30 s marked slow; slow tests are deletion candidates | Replace with the two suites: their purpose, the line for when an agent runs acceptance tests, the regression budget. The slow rule shrinks to one line (a regression test over 15 s fails the run) |
| `AGENTS.md` rendering item (line 6) | No full walks, one render per mode | Keep, as part of the regression rules |
| `AGENTS.md` (missing) | Nothing says what to run before a commit | Add: `test-apps` for app work, `test-framework` for framework work, never `test-all` or `cargo test` for app work unless asked |
| `docs/Build-Test.md` 337, 366-370 | test-all and `--slow` | The commands of both suites (`test-apps`, `test-framework`, `test-all`, `accept`, `accept-all`) |
| `docs/Build-Test.md` 372-388 "Slow tests" | The slow rules | Becomes "The two suites" and "The regression budget"; `slow` a short note |
| `docs/Build-Test.md` 391-402 rendering | No full walks, no screenshots | Keep, under the regression rules |
| `docs/dev/Practices-App-Dev.md` §7 (371-412) | No full walks; error-text checks; **growth:** a spec with a positive and a negative user for every service with an access rule; absence checks with a presence check; run the whole component before a commit; **growth:** phone layout checked at 390 px; **growth:** walks for scenarios a server spec cannot check | Rewrite to the new rules above: access checks per rule, not per service; the 390 px and scenario walks go to acceptance; before a commit, `test-apps` |
| `Practices-App-Dev.md` 112-113 | Each Mantle rule gets its own spec | Keep, worded as a rule test in Mantle |
| `Practices-App-Dev.md` §9 checklist (449, 453) | "the screen renders in tui"; **growth:** "`./moq test` for the app and Mantle ... and the walks run" | The regression suite within budget and lint; no walks |
| `Practices-App-Dev.md` 435 | Commit per phase naming the tests run | Keep |
| `docs/dev/Client-Testing.md` 54, 124, 186 | `--slow` and specs over 30 s | `--acceptance`, `acceptance/client/`, the 15 s limit |
| `docs/dev/Practices-Rust.md` 119-122, 138-139, 370 | Slow Rust tests ignored or opt-in; `cargo test -p` per crate touched | Keep `cargo test -p` per crate; the slow Rust tests become the framework's acceptance tests (`cargo test-slow`, the opt-in host tests) |
| `docs/ui/TUI.md` 348 | Gallery snapshots of every screen (`tui_screens`, opt-in) | An acceptance test of the framework |
| `docs/ui/Desktop-Client.md` 251-256, `Qvt-Client.md` 25 | Which commands cover what | The suite each belongs to |
| `plans/Comms-Framework.md` 1318-1327 | Browser, interop and slow runs only with permission | The acceptance suite, with its line |
| `runtime/dev-component/template/test/@Class@Screens.spec.js` | **growth:** renders every screen in qvt and tui, so every new component starts with an every-mode render test | Replace with one example rule spec in `test/`, and move a screen sweep example to `acceptance/` |
| `runtime/dev-component/template/README.md` 16, 24; `runtime/dev-component/README.md` 32-42 | `test/` covers services, screens and REST; run `./moq test` | The two suites and their commands |

**mr-boss** (`plan/` unless noted)

| Where | Now | Change |
|---|---|---|
| `impl-projects.md` §2 Done (47-50), which the later plans inherit | **growth:** AppStructure covering new screens, client walks passing, the phase's walk | The new Done: the phase's rule tests, the regression suite within budget, lint |
| `impl-drawers-commons.md` §4 Done (150-155), §7 (690-697) | **growth:** AppStructure extended to every Commons and Comms screen, the client walks | The same new Done; walks per milestone become acceptance |
| `impl-accounting.md` §2 Done (204-219), §8 (1545-1558), P21 (881-886) | **growth:** AppStructure, client walks, AccountingAccess extended every phase, 390 px; the full access walk | New Done; AccountingAccess per rule; the access walk and §8 walks are acceptance |
| `impl-warehouse.md` §2 Done (165-177), P18 (809-814), §8 | **growth:** AppStructure, walks with handheld at 390, WarehouseAccess every phase | Same |
| `impl-manufacturing.md` §2 Done (191-202), each phase's "Specs" (e.g. 303-306, 490-493, 599-603, 876-885), P17 (2666-2673), §8 | **growth:** AppStructure and TuiRender routes, ManufacturingAccess section per phase, a client walk per phase at 390 | Same; the phase sections' spec lists become rule tests plus named acceptance tests |
| `impl-products.md` §2 Done (119-130), 83-84, P14 (623-627), §8 | **growth:** AppStructure, walks, ProductsAccess every phase, the ProductsPhone pattern | Same |
| `impl-customer-service.md` §2 Done (267-282), §8 (1182, 1198-1200) | **growth:** AppStructure and TuiRender, walks, the access spec every phase; screenshots light and dark, PASS at two widths | Same; screenshots are dropped (already against the no-screenshot rule) |
| `impl-purchasing.md` §2 Done (230-245), §8 (1032, 1050-1052) | Same as above, with PurchasingAccess every phase; screenshots | Same |
| Every `impl-*.md`, each phase's "Specs." and "Usable when" | **growth:** each phase defines Shell and Access specs and usable-when scenarios, which §8 turns into tests | "Usable when" names acceptance tests; "Specs." names the rule tests only |
| `style-guide.md` 1022-1023 | Gallery.spec.js and GallerySmoke render every gallery screen | Acceptance tests of the framework's gallery |
| `design-doc-guide.md` 179, 285 | A "Stub spec" column | Keep; say it is a rule test |
| `README.md` 10-12 | Spec layout and the run commands | Both suites |

**mr-mantle, mr-pass, mr-marble**

| Where | Now | Change |
|---|---|---|
| mr-mantle `README.md` 27-28, `test/README.md` 10-16 | Run commands | Both suites |
| mr-mantle `plan/payments-shipping-edi.md` 385-390 | Fast specs; sandbox and live calls are deployment exercises | Already the split: the sandbox and live calls are acceptance tests |
| mr-pass `README.md` 39-44 | Spec lists, including `ForumHttp --slow` | ForumHttp and Scale in `acceptance/` |
| mr-pass `plan/design-forum.md` §19.1 (1527-1534), `design-shop.md` 1604 | Acceptance scenarios with `moqui test` and `test-client`, 390 px | The acceptance suite, by name |
| mr-marble `test/README.md` 24-31, 38, 58-63 | **growth:** SimpleScreensRender and MarbleScreens render every parameter-free screen; FindScreens every Find screen; BrowserSmoke | The sweeps move to acceptance; the regression suite keeps one smoke |

Also the briefs and skills agents are given: `.agents/skills/building/SKILL.md` says nothing about tests today; a short
`testing` skill (or a section of the building one) carries the two suites and the line for agents, so a brief need not
restate them. The worker and review briefs of a build name the suite of each test they ask for.

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
mr-boss server and its 69 walks. The opt-in host tests now run only with `--slow` (`moq`), so they are out of the
budget.

The counts here are what ran, not the ones in the brief that asked for this plan: 421 server specs (and 8 fixture
specs), 139 client specs, 4,919 features.

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
  spec. It builds the binary only when it is missing (as `--nobuild`) and uses `--slow` only when given. This is the
  command for application work, and the one `AGENTS.md` and `docs/dev/Practices-App-Dev.md` name for it.
- **`./moq test-framework`** (new): `cargo test`, `moq-test.sh`, and the specs and client specs of base-component and
  dev-component. This is for framework work.
- These are the regression suite; the acceptance suite has its own commands (`./moq accept`), split the same way by
  component.
- **`./moq test-all`** stays both in turn: before a release, after a framework change that apps depend on, or when
  asked.
- **The framework's own specs stay out of app runs:** a component spec run (`./moq test mr-boss`) already loads the
  framework's data but runs only that component's specs. `moqui test` with no component keeps its meaning (every
  component); `test-apps` is the narrower command.
- **Rule** (AGENTS.md): when only application components changed, run the app tests of the components touched while
  working, and `./moq test-apps` before a commit. Never run `cargo test`, the framework specs or test-all for
  application work unless asked.

Projected, with the cuts below:

| Run | Stages | Projected test time |
|---|---|--:|
| `test-apps` | moqui test over 4 components: load about 15 s, 94 specs (mr-boss 41, mr-mantle 44, mr-pass 7, mr-marble 2), 87 s of features; `--serve` with 3 servers (mr-boss, mr-pass, mr-marble), about 45 s to start, 6 walks, 43 s | about 3½ min |
| `test-framework` | cargo test about 1½ min; moq-test.sh; 18 specs of framework, basic-comms, tools, example, dev-aide and demo, about 8 s; 30 client specs (webroot 19 and dev-aide 4 offline; framework 5, basic-comms 1, example 1 served), 32 s plus their servers | about 3½ min |
| `test-all` | both | about 7 min |

## How the cut is made

- **Delete or move to acceptance.** Each "delete" row below is one of two: a duplicate (a render, user, mode or flag
  another kept test already checks) is deleted; a scenario or system check no regression test keeps (a plan's walk, an
  isolation walk, a screen sweep) moves to the acceptance suite instead, merged where several cover the same ground
  (one screen sweep per app in each mode it needs, one access and isolation walk per app). The next revision of this
  document marks every such row "acceptance" or "delete".

- **One render per screen and mode.** Where two specs render a screen in the same mode, the app's logic spec keeps the
  render and every check of it; the access spec checks refusals through service calls and transitions (or the status
  of a refused render), not by rendering the content again. The [duplicate renders](#duplicate-renders) table names the
  keeper for each.
- **More than one mode** only for a screen with its own qvt or tui template: mr-boss has 19 `.qvue` and 6 `.tui.json`,
  basic-comms 18 and 18, mr-marble 3 and 1, example 3 and 2, mr-pass 2 `.qvue`. The check says which template it
  covers. A `render-mode` section is a second mode only where the section differs.
- **Merge** means: carry over the checks the target lacks, at most two features' worth (one on average), as more
  conditions on a feature the target already has where one fits; then delete the file.
- **Caps.** A kept spec, with what is merged into it, has at most 4 features (5 for a client spec) unless its row names
  a number; the cap wins over the merges, so a spec that takes in many keeps one condition from each. The
  `test/suite.js` flows of mr-mantle keep their steps.
- **Authorization on and off.** mr-mantle has no `where: both` doubling: its specs turn authorization off once in
  `setup()`. The doubling is in mr-boss's access specs, whose features come in "with authz true" and "with authz false"
  pairs (WarehouseAccess 62, ManufacturingAccess 36, AccountingAccess 35, ProductsAccess 6). Group B drops it: each
  access feature runs once, with authorization on.
- **Fixtures** stay only where a kept walk needs them: 11 of the 12 mr-boss fixture specs go with their walks, which
  also shortens the mr-boss server start.
- **Slow** specs (the three fake-media call walks, mr-pass ForumHttp and Scale) stay marked slow and never run in a
  normal run; each is a candidate for deletion.

## Summary

By group (fixture specs are not counted as specs; their features are):

| Group | Specs now | Features now | Time now | Specs kept | Features kept | Projected time |
|---|--:|--:|--:|--:|--:|--:|
| A. Walk-everything renders | 32 | 585 | 350 s | 1 | 12 | 0 s |
| B. Access specs | 18 | 426 | 162 s | 6 | 40 | 10 s |
| C. Client walks | 145 | 798 | 783 s | 36 | 190 | 74 s |
| D. mr-boss app specs | 124 | 1066 | 190 s | 35 | 181 | 39 s |
| E. mr-mantle | 176 | 1574 | 210 s | 44 | 253 | 35 s |
| F. Small components | 65 | 470 | 34 s | 26 | 98 | 10 s |
| **All** | **560** | **4919** | **1729 s** | **148** | **774** | **168 s** |

By kind:

| Kind | Specs now | Features now | Feature time now | Specs kept | Features kept | Projected feature time |
|---|--:|--:|--:|--:|--:|--:|
| Server specs (`moqui test`, `moqui test demo`) | 421 | 4074 | 882 s | 112 | 607 | 95 s |
| Client specs, `--serve` | 99 | 425 | 814 s | 13 | 44 | 52 s |
| Client specs, `--offline` | 40 | 420 | 33 s | 23 | 123 | 22 s |

The targets were about 110 server specs, 45 client specs and 700 features. The plan keeps 112 server specs, 36 client
specs (13 served, 23 offline) and about 770 features; trimming the 19 webroot specs to 4 features each would reach 700,
at no saving in time.

By component:

| Component | Stage | Specs now | Time now | Specs kept | Projected time |
|---|---|--:|--:|--:|--:|
| basic-comms | client | 7 | 195 s | 1 | 4 s |
| basic-comms | server | 9 | 5 s | 3 | 1 s |
| demo | server | 1 | 0 s | 1 | 0 s |
| dev-aide | client | 7 | 0 s | 4 | 0 s |
| dev-aide | server | 5 | 12 s | 2 | 5 s |
| example | client | 2 | 17 s | 1 | 5 s |
| example | server | 7 | 3 s | 3 | 0 s |
| framework | client | 15 | 15 s | 5 | 1 s |
| framework | server | 24 | 6 s | 7 | 1 s |
| mr-boss | client | 69 | 503 s | 4 | 30 s |
| mr-boss | server | 172 | 616 s | 41 | 49 s |
| mr-mantle | server | 176 | 210 s | 44 | 35 s |
| mr-marble | client | 2 | 32 s | 1 | 4 s |
| mr-marble | server | 6 | 17 s | 2 | 0 s |
| mr-pass | client | 3 | 22 s | 1 | 9 s |
| mr-pass | server | 17 | 11 s | 7 | 3 s |
| tools | client | 1 | 30 s | 0 | 0 s |
| tools | server | 4 | 2 s | 2 | 1 s |
| webroot | client | 33 | 33 s | 19 | 22 s |

## Projected stage times

| Stage | Now | Projected | Budget | How |
|---|--:|--:|--:|---|
| `cargo test --workspace` | 97 s | about 1½ min | 1½ min | unchanged; the three golden failures of the run are a separate fix |
| `moqui test` | 944 s | about 2 min | 4 min | 15 s load, 112 specs × 0.15 s setup, 95 s of features |
| demo, offline, `moq-test.sh` | 81 s + moq-test.sh | about 1 min | 1 min | demo 15 s; webroot 19 specs × 0.8 s + 22 s; dev-aide 5 s |
| `--serve` | 1,014 s | about 2½ min | 3 min | 6 servers (tools has no client spec left) × about 15 s, 13 specs × 0.8 s, 52 s of features |
| **test-all, tests only** | **about 37 min** | **about 7 min** | **10 min (goal)** | builds not counted |

A projected time is the spec's time scaled by the features it keeps, plus a quarter of the time of each spec merged into
it; a rewritten access spec is 0.3 s a feature. The headroom under the budget is there for the merges costing more than
that. The budget is the time of the tests alone: the builds of the run measured (a 7 min 53 s debug test build, 4 min 19 s
for the opt-in host tests, now only with `--slow`, and 2 min 15 s of release build) are not in it.

## Order of work

Each step is one change, checked by running the specs of the area it touches (never the slow ones):

0. The two suites and the budget first, in mr-moqui: `acceptance/` discovery and `--acceptance` in `moqui test` and
   `moqui test-client`, `./moq accept`, `accept-all`, `test-apps` and `test-framework`, the regression budget and the
   15 s spec limit failing a run, one server for a run's served specs, and the instruction updates
   ([below](#instructions-to-update)). Then the specs marked slow move to `acceptance/` or are deleted.

1. Group A: plain deletions, about 285 s of `moqui test` and the 66 s of Monitoring, FindScreens and GallerySmoke.
2. Group C: the client walks and their fixtures, about 500 s of `--serve` besides the slow call walks. The mr-mantle and mr-marble server runs
   `SimpleScreensRender` as a `--spec` before serving; check that BrowserSmoke needs nothing that spec writes before
   deleting it.
3. Group B: the six access rewrites, one per app.
4. Group D: one app at a time.
5. Group E: one mr-mantle area (subdirectory) at a time.
6. Group F.

Projects and Commons have no client walk today, so they keep none; a golden-path walk for each is new work, not part of
this cut.

## A. Walk-everything renders

| Spec | Time | Features | Decision | Reason |
|---|--:|--:|---|---|
| `basic-comms/client/ShellCalls` | 0.5 | 2 | delete | desktop shell bundle renders; the desktop crate tests its bundle |
| `basic-comms/client/ShellComms` | 0.8 | 3 | delete | desktop shell bundle renders; the desktop crate tests its bundle |
| `demo/Demo` | 0.2 | 56 | keep, trim to 12 | the demo lock-down; drop the loops over every stubbed screen, keep one per rule |
| `dev-aide/DevAideScreens` | 1.2 | 9 | delete | Home and Settings render; the client specs cover the workspace |
| `example/Gallery` | 2.1 | 59 | delete | every gallery screen in its render mode; the archetypes are the goldens' |
| `example/client/GallerySmoke` | 12.3 | 23 | delete | the same gallery screens again in the /qapps shell |
| `mr-boss/Accounting/AccountingScreens` | 10.8 | 3 | delete | every Accounting route; walks every screen (or every route) of the app; rendering as such is the framework's goldens |
| `mr-boss/Accounting/AccountingServerStatic` | 14.7 | 1 | delete | every screen server-static in qvt; the qvt runtime is the webroot specs'; marked slow |
| `mr-boss/Accounting/AccountingShell` | 3.9 | 45 | delete | per-app shell walk: launcher and menus render once in the kept specs' first page |
| `mr-boss/AppStructure` | 51.7 | 18 | delete | every app root and default screen; marked slow; deleted for good |
| `mr-boss/Manufacturing/ManufacturingServerStatic` | 7.0 | 1 | delete | every screen server-static in qvt; the qvt runtime is the webroot specs'; marked slow |
| `mr-boss/Manufacturing/ManufacturingShell` | 1.5 | 20 | delete | per-app shell walk: launcher and menus render once in the kept specs' first page |
| `mr-boss/Products/ProductsPriceScreens` | 5.6 | 12 | delete | screen loop over Prices; checks of value live in ProductsPricing |
| `mr-boss/Products/ProductsScreens` | 20.0 | 37 | delete | screen loop over Products; walks every screen (or every route) of the app; rendering as such is the framework's goldens |
| `mr-boss/Products/ProductsShell` | 1.4 | 13 | delete | per-app shell walk: launcher and menus render once in the kept specs' first page |
| `mr-boss/Products/ProductsTaxonomyScreens` | 29.2 | 58 | delete | screen loop, 58 features, 29 s |
| `mr-boss/Products/ProductsVariantScreens` | 6.0 | 13 | delete | screen loop over variants; ProductsCatalog keeps the variant rules |
| `mr-boss/Projects/ProjectsServerStatic` | 12.8 | 1 | delete | every screen server-static in qvt; the qvt runtime is the webroot specs'; marked slow |
| `mr-boss/Projects/ProjectsShell` | 0.4 | 4 | delete | per-app shell walk: launcher and menus render once in the kept specs' first page |
| `mr-boss/Projects/ProjectsTui` | 5.4 | 7 | delete | every Projects screen in tui; only the screens with a .tui.json keep a tui render |
| `mr-boss/ServerStatic` | 31.6 | 1 | delete | every screen server-static in qvt; the qvt runtime is the webroot specs'; marked slow |
| `mr-boss/Shell` | 4.6 | 4 | delete | the suite shell's IronStone part; shellData is the framework's and Commons' first render |
| `mr-boss/TuiRender` | 47.5 | 3 | delete | every Commons/Comms/Insights screen in tui; marked slow |
| `mr-boss/Warehouse/WarehouseServerStatic` | 6.7 | 1 | delete | every screen server-static in qvt; the qvt runtime is the webroot specs'; marked slow |
| `mr-boss/Warehouse/WarehouseShell` | 1.6 | 25 | delete | per-app shell walk: launcher and menus render once in the kept specs' first page |
| `mr-marble/GitScreens` | 0.7 | 8 | delete | renders of the git screens; the git services are example GitHosting's |
| `mr-marble/MarbleScreens` | 3.9 | 18 | delete | the dashboard and every SimpleScreens page mounted under it |
| `mr-marble/MyAccountScreens` | 0.7 | 18 | delete | port of MyAccountScreenTests: every My Account screen |
| `mr-marble/SimpleScreensRender` | 11.6 | 59 | delete | port of PopcAdminScreenTests: every SimpleScreens page |
| `mr-marble/client/FindScreens` | 24.1 | 34 | delete | port of random-screen.spec: every Find screen in Chromium, 24 s |
| `tools/EdgeScreens` | 0.2 | 10 | delete | renders of the edge screens, 5 of 10 features skip without an edge node |
| `tools/client/Monitoring` | 29.6 | 19 | delete | walks the Monitor screens in Chromium, 30 s; MonitorSettings keeps the services |
| **Subtotal** | **350 s** | **585** | **1 kept** | **projected 12 features, 0 s** |

## B. Access specs

One access spec per app, 6 to 8 features, each run once with authorization on: who may open the app, the organization
(or facility, plant, client) scope, one refused write, one refused read of another organization's record, and the
standalone case. Projects' access spec is ClientIsolation, renamed ProjectsAccess. Commons and Comms have no access spec
of their own; AppOrganizationFilters covers their organization filter.

| Spec | Time | Features | Decision | Reason |
|---|--:|--:|---|---|
| `mr-boss/Accounting/AccountingAccess` | 14.5 | 61 | keep, rewrite to 7 | the Accounting access spec: drop the authz true/false doubling (35 features in pairs) |
| `mr-boss/Accounting/AccountingAccessWalk` | 43.4 | 15 | merge into AccountingAccess | marked slow, 43 s; one group's refusal goes into AccountingAccess |
| `mr-boss/Accounting/AccountingIsolation` | 0.5 | 4 | merge into AccountingAccess | organization isolation: one feature in AccountingAccess |
| `mr-boss/AppOrganizationFilters` | 0.2 | 25 | keep, trim to 6 | the user organization filter across the apps: one case per app |
| `mr-boss/Manufacturing/ManufacturingAccess` | 15.3 | 94 | keep, rewrite to 7 | the Manufacturing access spec: 94 features down to 7 |
| `mr-boss/Manufacturing/ManufacturingAccessWalk` | 17.4 | 5 | merge into ManufacturingAccess | the access walk, 17 s |
| `mr-boss/Manufacturing/ManufacturingCostIsolationWalk` | 8.0 | 2 | merge into ManufacturingAccess | cost and other-app isolation, 8 s |
| `mr-boss/Manufacturing/ManufacturingIsolationWalk` | 16.8 | 3 | merge into ManufacturingAccess | the isolation walk, 17 s |
| `mr-boss/Products/ProductsAccess` | 1.5 | 24 | keep, rewrite to 6 | the Products access spec; drop the authz on/disabled doubling |
| `mr-boss/Projects/ClientAccessInternal` | 2.6 | 3 | merge into ClientIsolation | internal users unchanged by client access: one feature |
| `mr-boss/Projects/ClientChatDrive` | 0.8 | 12 | merge into ClientIsolation | client chat and drive scope: one feature |
| `mr-boss/Projects/ClientCode` | 2.2 | 5 | merge into ClientIsolation | shared repositories for clients: one feature |
| `mr-boss/Projects/ClientIsolation` | 5.8 | 16 | keep, rewrite to 7 | becomes ProjectsAccess: client isolation is Projects' access model |
| `mr-boss/Projects/ClientWalk` | 0.7 | 7 | merge into ClientIsolation | the client's walk renders Home, invoices and RequestDetail again |
| `mr-boss/Projects/ClientWrites` | 0.5 | 6 | merge into ClientIsolation | the client's own writes: one feature |
| `mr-boss/Warehouse/WarehouseAccess` | 11.2 | 136 | keep, rewrite to 7 | the Warehouse access spec: 136 features (62 authz pairs) down to 7 |
| `mr-boss/Warehouse/WarehouseAccessWalk` | 11.7 | 4 | merge into WarehouseAccess | the organization user's walk, 12 s |
| `mr-boss/Warehouse/WarehouseIsolationWalk` | 8.7 | 4 | merge into WarehouseAccess | the isolation walk, 9 s |
| **Subtotal** | **162 s** | **426** | **6 kept** | **projected 40 features, 10 s** |

## C. Client walks

mr-boss keeps one golden-path walk per app that has one (Accounting, Products, Warehouse, Manufacturing); webroot and
framework keep their core; basic-comms keeps one walk without media; mr-marble, mr-pass and example keep one each.

| Spec | Time | Features | Decision | Reason |
|---|--:|--:|---|---|
| `basic-comms/CallClientFixtures` | 0.1 | 1 | fixture | users of the slow call walks |
| `basic-comms/CryptoClientFixtures` | 0.0 | 1 | fixture | the room CryptoFlow needs |
| `basic-comms/client/CallFlow` | 61.2 | 1 | keep (slow) | fake-media call; times out at 60 s; marked slow |
| `basic-comms/client/CallGuestFlow` | 60.7 | 1 | keep (slow) | fake-media guest call; marked slow |
| `basic-comms/client/CryptoFlow` | 1.7 | 1 | keep | the one non-media walk: crypto devices in two browsers, 1.7 s |
| `basic-comms/client/EncryptedCallFlow` | 61.4 | 1 | keep (slow) | fake-media SFrame call; marked slow |
| `basic-comms/client/EncryptedChatFlow` | 8.4 | 7 | merge into CryptoFlow | encrypted room: one message both ways onto CryptoFlow's devices |
| `dev-aide/client/DevAide` | 0.0 | 14 | keep, trim to 5 | the Workspace client |
| `dev-aide/client/DevAideBrowser` | 0.0 | 6 | merge into DevAide | the Artifact Browser layout |
| `dev-aide/client/DevAideEntity` | 0.0 | 6 | keep, trim to 5 | the entity operation builders, shared vectors with Rust |
| `dev-aide/client/DevAideGit` | 0.0 | 4 | merge into DevAide | workspace git depth |
| `dev-aide/client/DevAideQvue` | 0.0 | 4 | merge into DevAideScreen | the .qvue design view |
| `dev-aide/client/DevAideScreen` | 0.0 | 6 | keep, trim to 5 | the screen view and block tree |
| `dev-aide/client/DevAideService` | 0.0 | 4 | keep | the service writer and logic composer |
| `example/client/HandheldScan` | 5.0 | 3 | keep | the one example walk: handheld density at 390 px |
| `framework/ClientFixtures` | 0.1 | 16 | fixture | the users the framework client specs sign in as |
| `framework/client/LoadBasic` | 10.2 | 3 | delete | a 10 s load run with no assertion beyond 0 errors; run by hand |
| `framework/client/SecurityAccess` | 0.5 | 13 | keep, trim to 5 | A01 over HTTP, the core of the security proofs |
| `framework/client/SecurityAuthn` | 0.1 | 9 | keep, trim to 5 | A07 authentication |
| `framework/client/SecurityAuthnMore` | 0.8 | 21 | merge into SecurityAuthn | the A07 cases SecurityAuthn lacks: keep the two that matter |
| `framework/client/SecurityCors` | 0.0 | 8 | merge into SecurityHeaders | CORS: one allowed and one refused origin |
| `framework/client/SecurityErrors` | 0.0 | 1 | merge into SecurityHeaders | A10, one feature |
| `framework/client/SecurityHeaders` | 0.1 | 5 | keep, trim to 5 | A02 headers |
| `framework/client/SecurityMessages` | 0.0 | 8 | merge into SecurityAccess | /rest/sm access: one feature |
| `framework/client/SecurityMisc` | 0.2 | 9 | merge into SecurityHeaders | leftover A01/A05/A10 proofs |
| `framework/client/SecurityProxy` | 0.1 | 7 | merge into SecurityAccess | proxy access: one feature |
| `framework/client/SecurityRest` | 0.9 | 22 | keep, trim to 5 | A01 for REST |
| `framework/client/SecurityScreens` | 1.3 | 25 | keep, trim to 5 | A01 for screens |
| `framework/client/SecurityStatus` | 0.0 | 3 | merge into SecurityAccess | /status access: one feature |
| `framework/client/SecurityTarpit` | 1.0 | 2 | delete | skips unless the load-test conf is on; a load-test case, not a check |
| `framework/client/SecurityWebSocket` | 0.1 | 4 | merge into SecurityAccess | /notws access: one feature |
| `mr-boss/Accounting/AccountingClientFixtures` | 0.0 | 1 | fixture | the users the Accounting walk signs in as |
| `mr-boss/Manufacturing/ManufacturingPlanningFixtures` | 0.1 | 1 | delete | fixture of the deleted PlanningWalk; also shortens the mr-boss server start |
| `mr-boss/Manufacturing/ManufacturingScheduleFixtures` | 0.5 | 1 | delete | fixture of the deleted ScheduleWalk; also shortens the mr-boss server start |
| `mr-boss/Manufacturing/ManufacturingShopFloorFixtures` | 0.4 | 1 | delete | fixture of the deleted ShopFloorWalk; also shortens the mr-boss server start |
| `mr-boss/Manufacturing/ManufacturingSubcontractFixtures` | 0.1 | 1 | delete | fixture of the deleted SubcontractWalk; also shortens the mr-boss server start |
| `mr-boss/Products/ProductsCostFixtures` | 0.4 | 1 | delete | fixture of the deleted CostWalk; also shortens the mr-boss server start |
| `mr-boss/Products/ProductsImportFixtures` | 0.1 | 1 | delete | fixture of the deleted ImportWalk; also shortens the mr-boss server start |
| `mr-boss/Products/ProductsInventoryFixtures` | 0.0 | 1 | delete | fixture of the deleted InventoryWalk; also shortens the mr-boss server start |
| `mr-boss/Products/ProductsMethodFixtures` | 0.4 | 1 | delete | fixture of the deleted MethodWalk; also shortens the mr-boss server start |
| `mr-boss/Products/ProductsPriceFixtures` | 0.0 | 1 | delete | fixture of the deleted PriceWalk; also shortens the mr-boss server start |
| `mr-boss/Products/ProductsToolsFixtures` | 0.0 | 1 | delete | fixture of the deleted ToolsWalk; also shortens the mr-boss server start |
| `mr-boss/Warehouse/WarehouseProductionFixtures` | 0.4 | 1 | delete | fixture of the deleted ProductionWalk; also shortens the mr-boss server start |
| `mr-boss/client/Accounting/AccountingBankingWalk` | 19.6 | 3 | delete | one walk per app: Accounting keeps its golden path; the server spec keeps the rule |
| `mr-boss/client/Accounting/AccountingBudgetsWalk` | 10.4 | 3 | delete | one walk per app: Accounting keeps its golden path; the server spec keeps the rule |
| `mr-boss/client/Accounting/AccountingCloseWalk` | 10.5 | 1 | delete | one walk per app: Accounting keeps its golden path; the server spec keeps the rule |
| `mr-boss/client/Accounting/AccountingCollectionsWalk` | 6.2 | 2 | delete | one walk per app: Accounting keeps its golden path; the server spec keeps the rule |
| `mr-boss/client/Accounting/AccountingCurrencyWalk` | 4.7 | 1 | delete | one walk per app: Accounting keeps its golden path; the server spec keeps the rule |
| `mr-boss/client/Accounting/AccountingFixedAssetsWalk` | 4.5 | 1 | delete | one walk per app: Accounting keeps its golden path; the server spec keeps the rule |
| `mr-boss/client/Accounting/AccountingLedgerWalk` | 7.7 | 1 | delete | one walk per app: Accounting keeps its golden path; the server spec keeps the rule |
| `mr-boss/client/Accounting/AccountingMatchingWalk` | 5.9 | 1 | delete | one walk per app: Accounting keeps its golden path; the server spec keeps the rule |
| `mr-boss/client/Accounting/AccountingPayablesWalk` | 10.6 | 1 | delete | one walk per app: Accounting keeps its golden path; the server spec keeps the rule |
| `mr-boss/client/Accounting/AccountingPaymentGatewayWalk` | 7.7 | 1 | delete | one walk per app: Accounting keeps its golden path; the server spec keeps the rule |
| `mr-boss/client/Accounting/AccountingPaymentRunsWalk` | 9.6 | 2 | delete | one walk per app: Accounting keeps its golden path; the server spec keeps the rule |
| `mr-boss/client/Accounting/AccountingPaymentsWalk` | 13.0 | 1 | delete | one walk per app: Accounting keeps its golden path; the server spec keeps the rule |
| `mr-boss/client/Accounting/AccountingReceivablesWalk` | 9.3 | 1 | keep | the app's golden-path walk: invoice, send, payment received |
| `mr-boss/client/Accounting/AccountingReportsWalk` | 6.0 | 3 | delete | one walk per app: Accounting keeps its golden path; the server spec keeps the rule |
| `mr-boss/client/Accounting/AccountingReturnsWalk` | 3.5 | 1 | delete | one walk per app: Accounting keeps its golden path; the server spec keeps the rule |
| `mr-boss/client/Accounting/AccountingSettingsWalk` | 12.0 | 1 | delete | one walk per app: Accounting keeps its golden path; the server spec keeps the rule |
| `mr-boss/client/Accounting/AccountingSettlementWalk` | 11.9 | 1 | delete | one walk per app: Accounting keeps its golden path; the server spec keeps the rule |
| `mr-boss/client/Accounting/AccountingSetupWalk` | 6.8 | 1 | delete | one walk per app: Accounting keeps its golden path; the server spec keeps the rule |
| `mr-boss/client/Accounting/AccountingTaxWalk` | 16.8 | 3 | delete | one walk per app: Accounting keeps its golden path; the server spec keeps the rule |
| `mr-boss/client/Accounting/AccountingValuationWalk` | 5.4 | 1 | delete | one walk per app: Accounting keeps its golden path; the server spec keeps the rule |
| `mr-boss/client/Manufacturing/ManufacturingChangesWalk` | 3.2 | 4 | delete | one walk per app: Manufacturing keeps its golden path; the server spec keeps the rule |
| `mr-boss/client/Manufacturing/ManufacturingCostCloseWalk` | 1.9 | 4 | delete | one walk per app: Manufacturing keeps its golden path; the server spec keeps the rule |
| `mr-boss/client/Manufacturing/ManufacturingCostsWalk` | 2.4 | 3 | delete | one walk per app: Manufacturing keeps its golden path; the server spec keeps the rule |
| `mr-boss/client/Manufacturing/ManufacturingDowntimeWalk` | 5.5 | 2 | delete | one walk per app: Manufacturing keeps its golden path; the server spec keeps the rule |
| `mr-boss/client/Manufacturing/ManufacturingMaterialsWalk` | 1.6 | 2 | delete | one walk per app: Manufacturing keeps its golden path; the server spec keeps the rule |
| `mr-boss/client/Manufacturing/ManufacturingMethodsWalk` | 7.1 | 5 | delete | one walk per app: Manufacturing keeps its golden path; the server spec keeps the rule |
| `mr-boss/client/Manufacturing/ManufacturingPlanningWalk` | 4.1 | 3 | delete | one walk per app: Manufacturing keeps its golden path; the server spec keeps the rule |
| `mr-boss/client/Manufacturing/ManufacturingPolicyWalk` | 6.6 | 2 | delete | one walk per app: Manufacturing keeps its golden path; the server spec keeps the rule |
| `mr-boss/client/Manufacturing/ManufacturingQualityWalk` | 2.2 | 3 | delete | one walk per app: Manufacturing keeps its golden path; the server spec keeps the rule |
| `mr-boss/client/Manufacturing/ManufacturingQuantitiesWalk` | 2.1 | 3 | delete | one walk per app: Manufacturing keeps its golden path; the server spec keeps the rule |
| `mr-boss/client/Manufacturing/ManufacturingReportsWalk` | 2.1 | 3 | delete | one walk per app: Manufacturing keeps its golden path; the server spec keeps the rule |
| `mr-boss/client/Manufacturing/ManufacturingScheduleWalk` | 2.8 | 3 | delete | one walk per app: Manufacturing keeps its golden path; the server spec keeps the rule |
| `mr-boss/client/Manufacturing/ManufacturingShopFloorWalk` | 19.8 | 2 | delete | one walk per app: Manufacturing keeps its golden path; the server spec keeps the rule |
| `mr-boss/client/Manufacturing/ManufacturingSubcontractWalk` | 6.0 | 4 | delete | one walk per app: Manufacturing keeps its golden path; the server spec keeps the rule |
| `mr-boss/client/Manufacturing/ManufacturingWorkCentersWalk` | 8.4 | 3 | delete | one walk per app: Manufacturing keeps its golden path; the server spec keeps the rule |
| `mr-boss/client/Manufacturing/ManufacturingWorkOrdersWalk` | 3.1 | 2 | keep | the app's golden-path walk: new work order to released |
| `mr-boss/client/Products/ProductsCatalogSettings` | 2.3 | 1 | delete | one walk per app: Products keeps its golden path; the server spec keeps the rule |
| `mr-boss/client/Products/ProductsChecks` | 2.3 | 1 | delete | one walk per app: Products keeps its golden path; the server spec keeps the rule |
| `mr-boss/client/Products/ProductsCostWalk` | 7.5 | 1 | delete | one walk per app: Products keeps its golden path; the server spec keeps the rule |
| `mr-boss/client/Products/ProductsDocumentWalk` | 9.5 | 2 | delete | one walk per app: Products keeps its golden path; the server spec keeps the rule |
| `mr-boss/client/Products/ProductsFinish` | 3.0 | 3 | delete | one walk per app: Products keeps its golden path; the server spec keeps the rule |
| `mr-boss/client/Products/ProductsImportWalk` | 4.4 | 2 | delete | one walk per app: Products keeps its golden path; the server spec keeps the rule |
| `mr-boss/client/Products/ProductsInventoryWalk` | 6.7 | 2 | delete | one walk per app: Products keeps its golden path; the server spec keeps the rule |
| `mr-boss/client/Products/ProductsMethodWalk` | 21.2 | 1 | delete | one walk per app: Products keeps its golden path; the server spec keeps the rule |
| `mr-boss/client/Products/ProductsPhone` | 8.0 | 9 | delete | one walk per app: Products keeps its golden path; the server spec keeps the rule |
| `mr-boss/client/Products/ProductsPriceListWalk` | 6.3 | 1 | delete | one walk per app: Products keeps its golden path; the server spec keeps the rule |
| `mr-boss/client/Products/ProductsPriceWalk` | 10.1 | 2 | delete | one walk per app: Products keeps its golden path; the server spec keeps the rule |
| `mr-boss/client/Products/ProductsSupplierWalk` | 8.1 | 2 | delete | one walk per app: Products keeps its golden path; the server spec keeps the rule |
| `mr-boss/client/Products/ProductsTaxonomyWalk` | 8.8 | 1 | delete | one walk per app: Products keeps its golden path; the server spec keeps the rule |
| `mr-boss/client/Products/ProductsToolsWalk` | 10.0 | 4 | delete | one walk per app: Products keeps its golden path; the server spec keeps the rule |
| `mr-boss/client/Products/ProductsVariantWalk` | 6.7 | 2 | delete | one walk per app: Products keeps its golden path; the server spec keeps the rule |
| `mr-boss/client/Products/ProductsWalk` | 7.7 | 1 | keep | the app's golden-path walk: editor form submit, lifecycle, child navigation |
| `mr-boss/client/Warehouse/WarehouseAdjustmentsWalk` | 4.7 | 4 | delete | one walk per app: Warehouse keeps its golden path; the server spec keeps the rule |
| `mr-boss/client/Warehouse/WarehouseCountsWalk` | 2.7 | 3 | delete | one walk per app: Warehouse keeps its golden path; the server spec keeps the rule |
| `mr-boss/client/Warehouse/WarehouseFacilitiesWalk` | 16.0 | 4 | delete | one walk per app: Warehouse keeps its golden path; the server spec keeps the rule |
| `mr-boss/client/Warehouse/WarehouseFoundationWalk` | 3.3 | 3 | delete | one walk per app: Warehouse keeps its golden path; the server spec keeps the rule |
| `mr-boss/client/Warehouse/WarehouseInspectionWalk` | 7.3 | 3 | delete | one walk per app: Warehouse keeps its golden path; the server spec keeps the rule |
| `mr-boss/client/Warehouse/WarehouseInventoryWalk` | 5.1 | 4 | delete | one walk per app: Warehouse keeps its golden path; the server spec keeps the rule |
| `mr-boss/client/Warehouse/WarehouseMovesWalk` | 5.8 | 4 | delete | one walk per app: Warehouse keeps its golden path; the server spec keeps the rule |
| `mr-boss/client/Warehouse/WarehousePackingWalk` | 11.6 | 4 | delete | one walk per app: Warehouse keeps its golden path; the server spec keeps the rule |
| `mr-boss/client/Warehouse/WarehousePickingWalk` | 3.4 | 3 | delete | one walk per app: Warehouse keeps its golden path; the server spec keeps the rule |
| `mr-boss/client/Warehouse/WarehousePoliciesWalk` | 8.6 | 4 | delete | one walk per app: Warehouse keeps its golden path; the server spec keeps the rule |
| `mr-boss/client/Warehouse/WarehouseProductionWalk` | 3.8 | 3 | delete | one walk per app: Warehouse keeps its golden path; the server spec keeps the rule |
| `mr-boss/client/Warehouse/WarehousePutawayWalk` | 3.0 | 3 | delete | one walk per app: Warehouse keeps its golden path; the server spec keeps the rule |
| `mr-boss/client/Warehouse/WarehouseReceivingWalk` | 9.5 | 3 | keep | the app's golden-path walk: receive a purchase order part, short receipt |
| `mr-boss/client/Warehouse/WarehouseReportsWalk` | 7.0 | 5 | delete | one walk per app: Warehouse keeps its golden path; the server spec keeps the rule |
| `mr-boss/client/Warehouse/WarehouseReturnsWalk` | 8.3 | 3 | delete | one walk per app: Warehouse keeps its golden path; the server spec keeps the rule |
| `mr-boss/client/Warehouse/WarehouseShippingWalk` | 3.7 | 3 | delete | one walk per app: Warehouse keeps its golden path; the server spec keeps the rule |
| `mr-boss/client/Warehouse/WarehouseTransfersWalk` | 14.9 | 4 | delete | one walk per app: Warehouse keeps its golden path; the server spec keeps the rule |
| `mr-marble/ClientAccess` | 0.0 | 1 | fixture | the access BrowserSmoke needs |
| `mr-marble/client/BrowserSmoke` | 8.2 | 7 | keep, trim to 3 | the one mr-marble walk: the flow data pages, PrintOrder's SVG |
| `mr-pass/Billing/PortalFixtures` | 0.4 | 1 | fixture | PortalHttp's data |
| `mr-pass/ClientFixtures` | 0.1 | 1 | fixture | the HTTP test community |
| `mr-pass/RemoteFixtures` | 0.0 | 1 | fixture | ignored outside the remote frontend runtime |
| `mr-pass/client/ForumHttp` | 0.0 | 1 | keep (slow) | marked slow (64 s) |
| `mr-pass/client/PortalHttp` | 4.1 | 3 | keep | the one mr-pass walk: Billing and Supplier over HTTP |
| `mr-pass/client/RemoteHttp` | 18.3 | 6 | merge into PortalHttp | the remote frontend over HTTP, 18 s; one feature |
| `webroot/client/Dialog` | 1.1 | 1 | merge into moqui-html | one DOM feature |
| `webroot/client/DisabledControls` | 1.2 | 1 | merge into moqui-html | one DOM feature |
| `webroot/client/Download` | 1.0 | 1 | merge into moqui-html | one DOM feature |
| `webroot/client/HeadClient` | 1.0 | 1 | merge into moqui-html | one DOM feature |
| `webroot/client/InlineSelect` | 1.0 | 1 | merge into moqui-html | one DOM feature |
| `webroot/client/Keys` | 8.1 | 3 | keep | webroot core: html keys with real key events |
| `webroot/client/MoquiHtmlCss` | 3.9 | 7 | keep, trim to 5 | webroot core: layout metrics |
| `webroot/client/MoquiL10n` | 0.0 | 7 | keep, trim to 5 | webroot core |
| `webroot/client/MoquiLib` | 0.2 | 29 | keep, trim to 5 | webroot core |
| `webroot/client/QvtKeys` | 10.3 | 3 | keep | webroot core: qvt keys, 10 s; trim the waits |
| `webroot/client/TablePopup` | 0.9 | 1 | merge into moqui-html | one DOM feature |
| `webroot/client/WebrootAssist` | 0.1 | 30 | keep, trim to 5 | webroot core: Assist |
| `webroot/client/WebrootAssistCanvas` | 0.0 | 16 | merge into WebrootAssist | the Assist canvas |
| `webroot/client/WebrootChat` | 0.2 | 22 | keep, trim to 5 | webroot core: the chat store |
| `webroot/client/WebrootCode` | 0.0 | 12 | merge into WebrootVue.qvt.markdown | vendored CodeMirror smoke |
| `webroot/client/WebrootPublic.qvt` | 0.0 | 4 | keep | webroot core: the public shell |
| `webroot/client/WebrootReport` | 0.2 | 46 | keep, trim to 20 | webroot core: reports; 46 features of pure functions |
| `webroot/client/WebrootRum` | 0.0 | 4 | keep | webroot core: real-user monitoring |
| `webroot/client/WebrootSFrame` | 0.0 | 7 | keep, trim to 5 | webroot core: SFrame on WebCrypto |
| `webroot/client/WebrootVue.qvt` | 0.1 | 25 | keep, trim to 5 | webroot core: the qvt components |
| `webroot/client/WebrootVue.qvt.code` | 0.0 | 4 | merge into WebrootVue.qvt.markdown | vendored highlight.js smoke |
| `webroot/client/WebrootVue.qvt.coedit` | 0.3 | 9 | merge into WebrootVue.qvt.markdown | vendored Tiptap smoke |
| `webroot/client/WebrootVue.qvt.htmlview` | 0.0 | 11 | merge into WebrootVue.qvt.views | a view kind |
| `webroot/client/WebrootVue.qvt.layout` | 0.0 | 5 | keep | webroot core: split and wizard |
| `webroot/client/WebrootVue.qvt.markdown` | 0.5 | 11 | keep, trim to 5 | webroot core: the vendored editors and viewers, one spec |
| `webroot/client/WebrootVue.qvt.records` | 0.0 | 19 | keep, trim to 5 | webroot core: records |
| `webroot/client/WebrootVue.qvt.shell` | 0.0 | 29 | keep, trim to 5 | webroot core: the suite shell |
| `webroot/client/WebrootVue.qvt.start` | 0.0 | 1 | merge into WebrootVue.qvt | one feature: what is set before the mount |
| `webroot/client/WebrootVue.qvt.static` | 0.1 | 26 | keep, trim to 5 | webroot core: server-static qvt |
| `webroot/client/WebrootVue.qvt.views` | 0.0 | 21 | keep, trim to 5 | webroot core: views |
| `webroot/client/WebrootVueCss` | 1.8 | 2 | merge into MoquiHtmlCss | computed styles, 2 features |
| `webroot/client/WebrootVueDisplay` | 1.1 | 1 | merge into WebrootVue.qvt | m-display is a qvt component |
| `webroot/client/moqui-html` | 0.0 | 16 | keep, trim to 5 | webroot core: the html client's functions |
| **Subtotal** | **783 s** | **798** | **36 kept** | **projected 190 features, 74 s** |

## D. mr-boss app specs

Four or five specs per app over the app's own logic, each with the merged specs' checks it lacks.

| Spec | Time | Features | Decision | Reason |
|---|--:|--:|---|---|
| `mr-boss/Accounting/AccountingBanking` | 0.8 | 4 | merge into AccountingPayments | the bank account screens |
| `mr-boss/Accounting/AccountingBudgets` | 0.5 | 4 | merge into AccountingPayables | a bill's budget override |
| `mr-boss/Accounting/AccountingCollections` | 2.2 | 6 | merge into AccountingReceivables | collections, credit, recurring |
| `mr-boss/Accounting/AccountingFixedAssets` | 0.8 | 3 | merge into AccountingPeriodClose | depreciation at close |
| `mr-boss/Accounting/AccountingIssuedFiles` | 0.4 | 3 | merge into AccountingReporting | issued evidence stays exact |
| `mr-boss/Accounting/AccountingMatching` | 0.8 | 4 | merge into AccountingPayables | BillDetail's Match tab |
| `mr-boss/Accounting/AccountingOutputs` | 0.5 | 6 | merge into AccountingReporting | printed outputs |
| `mr-boss/Accounting/AccountingPayables` | 1.7 | 8 | keep, trim to 6 | payables and approvals |
| `mr-boss/Accounting/AccountingPaymentFiles` | 0.8 | 6 | merge into AccountingPayments | payment authority on Files |
| `mr-boss/Accounting/AccountingPaymentGateway` | 1.2 | 4 | merge into AccountingPayments | gateway actions on the payment screens |
| `mr-boss/Accounting/AccountingPaymentRefunds` | 0.8 | 4 | merge into AccountingPayments | the refund dialog |
| `mr-boss/Accounting/AccountingPaymentRuns` | 2.0 | 6 | merge into AccountingPayments | payment runs |
| `mr-boss/Accounting/AccountingPaymentTransfers` | 1.3 | 7 | merge into AccountingPayments | transfers |
| `mr-boss/Accounting/AccountingPayments` | 2.5 | 15 | keep, trim to 6 | Accounting payments: the hub for payment logic |
| `mr-boss/Accounting/AccountingPeriodClose` | 0.8 | 4 | keep, trim to 5 | period close |
| `mr-boss/Accounting/AccountingProductionCostClose` | 0.9 | 3 | merge into AccountingPeriodClose | production cost close |
| `mr-boss/Accounting/AccountingReceivables` | 2.8 | 11 | keep, trim to 6 | receivables |
| `mr-boss/Accounting/AccountingReporting` | 4.6 | 12 | keep, trim to 6 | the report catalog per role |
| `mr-boss/Accounting/AccountingSettlement` | 1.1 | 9 | merge into AccountingReceivables | settlement drafts |
| `mr-boss/Accounting/AccountingTax` | 0.5 | 6 | merge into AccountingPeriodClose | the tax screens |
| `mr-boss/Accounting/AccountingValuation` | 0.6 | 3 | merge into AccountingPayables | landed cost on a freight bill |
| `mr-boss/Commons/Calendar` | 0.9 | 15 | keep, trim to 6 | calendar |
| `mr-boss/Commons/CalendarSettings` | 0.7 | 8 | merge into Calendar | calendar settings |
| `mr-boss/Commons/CommonsSettings` | 1.0 | 8 | merge into Tasks | Home's setup card |
| `mr-boss/Commons/ContactBookSettings` | 0.7 | 5 | merge into Contacts | contact book settings |
| `mr-boss/Commons/ContactImport` | 9.2 | 12 | merge into Contacts | the import wizard, 9 s |
| `mr-boss/Commons/Contacts` | 19.2 | 46 | keep, trim to 6 | contacts: 46 features, 19 s, down to 6 |
| `mr-boss/Commons/EventDetail` | 1.4 | 12 | merge into Calendar | event detail |
| `mr-boss/Commons/FileShares` | 0.2 | 4 | merge into Files | sharing |
| `mr-boss/Commons/Files` | 1.0 | 17 | keep, trim to 6 | files and drives |
| `mr-boss/Commons/Knowledge` | 2.4 | 21 | keep, trim to 6 | knowledge |
| `mr-boss/Commons/KnowledgeReview` | 0.9 | 5 | merge into Knowledge | review |
| `mr-boss/Commons/Notes` | 0.8 | 8 | merge into Knowledge | notes |
| `mr-boss/Commons/Notifications` | 0.6 | 10 | merge into Tasks | notifications |
| `mr-boss/Commons/QuickCreate` | 0.5 | 19 | merge into Tasks | QuickCreate: one kind, not every kind |
| `mr-boss/Commons/Requests` | 0.5 | 10 | merge into Tasks | requests |
| `mr-boss/Commons/Schedules` | 0.3 | 5 | merge into Calendar | schedules |
| `mr-boss/Commons/SyncSettings` | 0.2 | 4 | merge into Calendar | device sync addresses |
| `mr-boss/Commons/Tasks` | 2.1 | 15 | keep, trim to 6 | tasks |
| `mr-boss/Commons/TodayPanel` | 0.6 | 9 | merge into Calendar | Today |
| `mr-boss/Comms/CallTarget` | 0.1 | 4 | merge into Chat | the call-target provider |
| `mr-boss/Comms/Chat` | 0.7 | 19 | keep, trim to 6 | chat |
| `mr-boss/Comms/ChatPanel` | 0.1 | 2 | merge into Chat | the tray tab |
| `mr-boss/Comms/ChatSettings` | 0.1 | 5 | merge into Chat | chat settings |
| `mr-boss/Comms/CommsHome` | 1.0 | 5 | merge into Consent | Home's counters |
| `mr-boss/Comms/CommsModeration` | 0.6 | 3 | merge into Consent | moderation |
| `mr-boss/Comms/Compose` | 0.7 | 11 | merge into Inbox | the composer |
| `mr-boss/Comms/Consent` | 0.4 | 12 | keep, trim to 6 | consent |
| `mr-boss/Comms/Inbox` | 1.1 | 12 | keep, trim to 4 | the inbox |
| `mr-boss/Comms/InboxMailboxes` | 0.6 | 5 | merge into Inbox | mailboxes |
| `mr-boss/Comms/MailAccounts` | 1.7 | 18 | keep, trim to 6 | mail accounts |
| `mr-boss/Comms/MailSettings` | 0.9 | 10 | merge into MailAccounts | identities, rules, out of office |
| `mr-boss/Comms/MailSignIn` | 0.3 | 7 | merge into MailAccounts | OAuth sign-in |
| `mr-boss/Manufacturing/ManufacturingDowntime` | 1.3 | 6 | merge into ManufacturingShopFloor | downtime |
| `mr-boss/Manufacturing/ManufacturingMaterials` | 1.8 | 5 | merge into ManufacturingWorkOrders | materials |
| `mr-boss/Manufacturing/ManufacturingMethods` | 1.1 | 6 | merge into ManufacturingWorkOrders | methods |
| `mr-boss/Manufacturing/ManufacturingPlanning` | 0.9 | 5 | keep, trim to 4 | supply planning |
| `mr-boss/Manufacturing/ManufacturingReports` | 2.5 | 4 | keep | reports and Kpis |
| `mr-boss/Manufacturing/ManufacturingSchedule` | 1.2 | 6 | merge into ManufacturingPlanning | scheduling and capacity |
| `mr-boss/Manufacturing/ManufacturingShopFloor` | 2.3 | 6 | keep, trim to 4 | shop floor and production time |
| `mr-boss/Manufacturing/ManufacturingSubcontracting` | 1.0 | 5 | merge into ManufacturingPlanning | subcontracting |
| `mr-boss/Manufacturing/ManufacturingWorkCenters` | 0.6 | 5 | merge into ManufacturingShopFloor | work centers |
| `mr-boss/Manufacturing/ManufacturingWorkOrders` | 2.0 | 4 | keep, trim to 4 | work orders |
| `mr-boss/Products/ProductsCatalog` | 1.6 | 12 | keep, trim to 6 | the catalog and its capabilities |
| `mr-boss/Products/ProductsChecks` | 0.3 | 5 | merge into ProductsCatalog | catalog checks |
| `mr-boss/Products/ProductsCosts` | 2.5 | 15 | keep, trim to 6 | costs |
| `mr-boss/Products/ProductsDocuments` | 4.1 | 19 | merge into ProductsInventory | datasheets and pack labels |
| `mr-boss/Products/ProductsFinish` | 2.1 | 8 | merge into ProductsPricing | finishing a price list |
| `mr-boss/Products/ProductsImport` | 0.7 | 7 | merge into ProductsCatalog | import |
| `mr-boss/Products/ProductsInventory` | 5.0 | 13 | keep, trim to 6 | inventory |
| `mr-boss/Products/ProductsMethods` | 10.3 | 19 | merge into ProductsCosts | BOM and routing, 10 s |
| `mr-boss/Products/ProductsPriceLists` | 1.9 | 21 | merge into ProductsPricing | price lists |
| `mr-boss/Products/ProductsPricePrint` | 0.1 | 1 | merge into ProductsPricing | printed price list |
| `mr-boss/Products/ProductsPricing` | 3.7 | 13 | keep, trim to 6 | pricing |
| `mr-boss/Products/ProductsSharedContext` | 1.7 | 8 | merge into ProductsCatalog | shared context |
| `mr-boss/Products/ProductsSuppliers` | 7.1 | 22 | keep, trim to 6 | suppliers |
| `mr-boss/Products/ProductsTaxonomy` | 0.8 | 9 | merge into ProductsCatalog | categories and features |
| `mr-boss/Products/ProductsTools` | 5.2 | 21 | merge into ProductsInventory | tools |
| `mr-boss/Products/ProductsVariants` | 3.9 | 8 | merge into ProductsCatalog | variant rules |
| `mr-boss/Projects/Billing` | 0.8 | 5 | keep, trim to 4 | billing |
| `mr-boss/Projects/BillingWalk` | 0.8 | 6 | merge into Billing | server-side walk of billing |
| `mr-boss/Projects/ClientBilling` | 1.3 | 8 | merge into Billing | billing for clients |
| `mr-boss/Projects/Code` | 1.5 | 8 | keep, trim to 4 | code |
| `mr-boss/Projects/CodeReviewWalk` | 0.7 | 6 | merge into Code | server-side walk of review |
| `mr-boss/Projects/ColumnChooser` | 0.4 | 3 | merge into Reports | the column chooser on TimeEntries (new, 477f9e89) |
| `mr-boss/Projects/MergeRequests` | 1.2 | 7 | merge into Code | merge requests |
| `mr-boss/Projects/MilestoneBillingWalk` | 0.9 | 8 | merge into Billing | milestone billing |
| `mr-boss/Projects/NewProject` | 0.3 | 5 | merge into WorkItems | NewProject |
| `mr-boss/Projects/ProjectDetail` | 1.3 | 8 | merge into WorkItems | ProjectDetail tabs |
| `mr-boss/Projects/ProjectRequests` | 0.9 | 13 | merge into WorkItems | requests |
| `mr-boss/Projects/Projects` | 0.7 | 5 | merge into WorkItems | the project list and bulk |
| `mr-boss/Projects/ProjectsHome` | 0.6 | 3 | merge into WorkItems | Home |
| `mr-boss/Projects/ProjectsSettings` | 0.5 | 5 | merge into Reports | settings pages |
| `mr-boss/Projects/ReleaseWalk` | 1.3 | 6 | merge into WorkItems | server-side walk of releases |
| `mr-boss/Projects/Reports` | 1.4 | 7 | keep, trim to 4 | reports and outputs |
| `mr-boss/Projects/RequestWalk` | 0.5 | 6 | merge into WorkItems | server-side walk of requests |
| `mr-boss/Projects/ResourceWalk` | 0.6 | 4 | merge into TimeEntries | server-side walk of resources |
| `mr-boss/Projects/Resources` | 1.3 | 4 | merge into TimeEntries | resources |
| `mr-boss/Projects/SprintWalk` | 1.2 | 5 | merge into WorkItems | server-side walk of sprints |
| `mr-boss/Projects/TimeEntries` | 2.8 | 7 | keep, trim to 4 | time entries |
| `mr-boss/Projects/TimeWalk` | 2.4 | 6 | merge into TimeEntries | server-side walk of time |
| `mr-boss/Projects/TimelineWalk` | 2.2 | 8 | merge into WorkItems | server-side walk of the timeline |
| `mr-boss/Projects/WorkItemDetail` | 1.5 | 10 | merge into WorkItems | WorkItemDetail |
| `mr-boss/Projects/WorkItems` | 1.7 | 9 | keep, trim to 4 | work items and QuickCreate |
| `mr-boss/PublicShare` | 0.1 | 5 | keep, trim to 4 | the public link page |
| `mr-boss/PublishedCalendar` | 0.2 | 4 | merge into PublicShare | the other token route |
| `mr-boss/Shared/Chatter` | 1.1 | 27 | keep, trim to 6 | Chatter, used by every app: 27 features down to 6 |
| `mr-boss/Shared/RecordFiles` | 0.2 | 11 | merge into Chatter | RecordFiles beside Chatter |
| `mr-boss/Unsubscribe` | 0.1 | 8 | merge into Consent | the public unsubscribe page |
| `mr-boss/Warehouse/WarehouseAdjustments` | 1.2 | 7 | merge into WarehouseInventory | adjustments |
| `mr-boss/Warehouse/WarehouseCounts` | 1.4 | 7 | merge into WarehouseInventory | counts |
| `mr-boss/Warehouse/WarehouseFacilities` | 2.8 | 32 | keep, trim to 6 | facilities and locations: 32 features down to 6 |
| `mr-boss/Warehouse/WarehouseInspections` | 0.5 | 4 | merge into WarehouseReceiving | inspections |
| `mr-boss/Warehouse/WarehouseInventory` | 1.4 | 10 | keep, trim to 6 | inventory and traceability |
| `mr-boss/Warehouse/WarehouseMoves` | 0.7 | 6 | merge into WarehouseInventory | moves |
| `mr-boss/Warehouse/WarehousePacking` | 0.6 | 4 | merge into WarehouseShipping | packing |
| `mr-boss/Warehouse/WarehousePicking` | 0.3 | 3 | merge into WarehouseShipping | picking |
| `mr-boss/Warehouse/WarehousePolicies` | 1.8 | 18 | merge into WarehouseFacilities | policy screens |
| `mr-boss/Warehouse/WarehousePutaway` | 0.6 | 5 | merge into WarehouseReceiving | putaway |
| `mr-boss/Warehouse/WarehouseReceiving` | 0.9 | 5 | keep, trim to 4 | receiving |
| `mr-boss/Warehouse/WarehouseReports` | 0.9 | 7 | keep, trim to 4 | reports |
| `mr-boss/Warehouse/WarehouseReturns` | 0.8 | 6 | merge into WarehouseReceiving | returns |
| `mr-boss/Warehouse/WarehouseShipping` | 1.2 | 5 | keep, trim to 4 | shipping |
| `mr-boss/Warehouse/WarehouseTransfers` | 0.3 | 3 | merge into WarehouseInventory | transfer reads |
| **Subtotal** | **190 s** | **1066** | **35 kept** | **projected 181 features, 39 s** |

## E. mr-mantle

The eight `test/suite.js` flows are the backbone and keep every step. Each area (subdirectory) the flows do not reach
keeps one spec, 36 in all.

| Spec | Time | Features | Decision | Reason |
|---|--:|--:|---|---|
| `mr-mantle/RestApiTests` | 0.5 | 12 | keep | test/suite.js end-to-end flow: the backbone |
| `mr-mantle/account/AccountingBankReconciliation` | 1.1 | 11 | merge into AccountingPaymentLifecycle | same area as AccountingPaymentLifecycle |
| `mr-mantle/account/AccountingCollections` | 1.2 | 9 | keep, trim to 4 | collections and credit |
| `mr-mantle/account/AccountingCredit` | 1.4 | 9 | merge into AccountingCollections | same area as AccountingCollections |
| `mr-mantle/account/AccountingCustomerFinance` | 0.6 | 8 | merge into AccountingCollections | same area as AccountingCollections |
| `mr-mantle/account/AccountingDelivery` | 3.1 | 12 | merge into AccountingCollections | same area as AccountingCollections |
| `mr-mantle/account/AccountingInternalTransfers` | 0.9 | 4 | merge into AccountingPaymentLifecycle | same area as AccountingPaymentLifecycle |
| `mr-mantle/account/AccountingMatching` | 3.1 | 14 | merge into AccountingSettlement | same area as AccountingSettlement |
| `mr-mantle/account/AccountingPayables` | 1.5 | 10 | merge into AccountingPaymentRuns | same area as AccountingPaymentRuns |
| `mr-mantle/account/AccountingPaymentGateway` | 0.9 | 8 | merge into PaymentGateway | same area as PaymentGateway |
| `mr-mantle/account/AccountingPaymentLifecycle` | 2.4 | 14 | keep, trim to 6 | payments, the area the flows touch least |
| `mr-mantle/account/AccountingPaymentNotifications` | 0.4 | 4 | merge into AccountingPaymentLifecycle | same area as AccountingPaymentLifecycle |
| `mr-mantle/account/AccountingPaymentOutput` | 2.0 | 5 | merge into AccountingPaymentLifecycle | same area as AccountingPaymentLifecycle |
| `mr-mantle/account/AccountingPaymentRefunds` | 1.6 | 4 | merge into AccountingPaymentLifecycle | same area as AccountingPaymentLifecycle |
| `mr-mantle/account/AccountingPaymentRunControls` | 1.9 | 10 | merge into AccountingPaymentRuns | same area as AccountingPaymentRuns |
| `mr-mantle/account/AccountingPaymentRuns` | 1.0 | 8 | keep, trim to 4 | payment runs and bank files |
| `mr-mantle/account/AccountingPortals` | 2.9 | 17 | merge into AccountingCollections | same area as AccountingCollections |
| `mr-mantle/account/AccountingRecurringAging` | 0.8 | 7 | merge into AccountingCollections | same area as AccountingCollections |
| `mr-mantle/account/AccountingSettlement` | 2.3 | 16 | keep, trim to 6 | settlement and matching |
| `mr-mantle/account/AccountingSettlementConcurrency` | 1.1 | 5 | merge into AccountingSettlement | same area as AccountingSettlement |
| `mr-mantle/account/AccountingTermDueDays` | 0.4 | 3 | merge into AccountingPaymentRuns | same area as AccountingPaymentRuns |
| `mr-mantle/account/FinancialAccountServices` | 0.3 | 8 | merge into AccountingPaymentLifecycle | same area as AccountingPaymentLifecycle |
| `mr-mantle/account/GatewayContracts` | 0.2 | 6 | merge into PaymentGateway | same area as PaymentGateway |
| `mr-mantle/account/InvoiceOrderApply` | 0.3 | 8 | delete | order-to-invoice is OrderToCashBasicFlow's |
| `mr-mantle/account/InvoicePaymentMisc` | 1.0 | 12 | merge into AccountingPaymentLifecycle | same area as AccountingPaymentLifecycle |
| `mr-mantle/account/InvoicePostOrder` | 0.2 | 2 | delete | posting an order's invoice is OrderToCashBasicFlow's |
| `mr-mantle/account/PaymentGateway` | 0.3 | 8 | keep, trim to 4 | the gateway contract |
| `mr-mantle/account/PaymentMethodServices` | 0.1 | 7 | merge into AccountingPaymentLifecycle | same area as AccountingPaymentLifecycle |
| `mr-mantle/edi/EdiX12` | 2.1 | 17 | keep, trim to 6 | EDI X12 |
| `mr-mantle/ext/AuthorizeNetGateway` | 0.8 | 15 | keep, trim to 6 | a card gateway: one is enough |
| `mr-mantle/ext/BankFileServices` | 0.1 | 9 | merge into AccountingPaymentRuns | same area as AccountingPaymentRuns |
| `mr-mantle/ext/BankOutputFiles` | 0.3 | 13 | merge into AccountingPaymentRuns | same area as AccountingPaymentRuns |
| `mr-mantle/ext/PayTraceGateway` | 1.7 | 13 | merge into AuthorizeNetGateway | same area as AuthorizeNetGateway |
| `mr-mantle/ext/ShippoGateway` | 2.5 | 12 | keep, trim to 4 | the carrier gateway |
| `mr-mantle/facility/ContactAndFacilityServices` | 0.1 | 10 | merge into WarehouseOperationPolicy | same area as WarehouseOperationPolicy |
| `mr-mantle/facility/WarehouseFacilities` | 0.8 | 9 | merge into WarehouseOperationPolicy | same area as WarehouseOperationPolicy |
| `mr-mantle/facility/WarehouseOperationPolicy` | 1.1 | 21 | keep, trim to 6 | facilities and their policies, 21 features down to 6 |
| `mr-mantle/facility/WarehousePolicy` | 0.3 | 6 | merge into WarehouseOperationPolicy | same area as WarehouseOperationPolicy |
| `mr-mantle/facility/WarehousePutaway` | 1.7 | 8 | merge into WarehouseOperationPolicy | same area as WarehouseOperationPolicy |
| `mr-mantle/forum/Mailboxes` | 4.6 | 22 | keep, trim to 6 | forum mailboxes, 22 features down to 6 |
| `mr-mantle/humanres/HumanResServices` | 0.1 | 8 | merge into PayrollServices | same area as PayrollServices |
| `mr-mantle/humanres/PayrollAdjustments` | 0.3 | 9 | merge into PayrollServices | same area as PayrollServices |
| `mr-mantle/humanres/PayrollServices` | 0.2 | 10 | keep, trim to 4 | human resources |
| `mr-mantle/ledger/AccountingActivities` | 0.3 | 6 | keep | test/suite.js end-to-end flow: the backbone |
| `mr-mantle/ledger/AccountingApplicationRules` | 0.6 | 6 | merge into AccountingControls | same area as AccountingControls |
| `mr-mantle/ledger/AccountingBookDates` | 0.7 | 8 | merge into AccountingPeriodClose | same area as AccountingPeriodClose |
| `mr-mantle/ledger/AccountingControls` | 1.7 | 34 | keep, trim to 6 | GL controls, 34 features down to 6 |
| `mr-mantle/ledger/AccountingCurrency` | 1.4 | 15 | merge into AccountingControls | same area as AccountingControls |
| `mr-mantle/ledger/AccountingFiscalPeriodDays` | 0.0 | 5 | merge into AccountingPeriodClose | same area as AccountingPeriodClose |
| `mr-mantle/ledger/AccountingFixedAssets` | 1.4 | 11 | merge into AccountingValuation | same area as AccountingValuation |
| `mr-mantle/ledger/AccountingPaymentCorrections` | 4.1 | 9 | merge into AccountingPaymentLifecycle | same area as AccountingPaymentLifecycle |
| `mr-mantle/ledger/AccountingPeriodClose` | 2.3 | 8 | keep, trim to 4 | period close |
| `mr-mantle/ledger/AccountingProjectCost` | 0.6 | 4 | merge into AccountingValuation | same area as AccountingValuation |
| `mr-mantle/ledger/AccountingReceivables` | 2.1 | 22 | merge into AccountingControls | same area as AccountingControls |
| `mr-mantle/ledger/AccountingReports` | 0.0 | 3 | merge into FinancialReports | same area as FinancialReports |
| `mr-mantle/ledger/AccountingSetup` | 0.9 | 7 | merge into AccountingControls | same area as AccountingControls |
| `mr-mantle/ledger/AccountingValuation` | 2.3 | 15 | keep, trim to 6 | valuation and fixed assets |
| `mr-mantle/ledger/FinancialReports` | 1.1 | 6 | keep, trim to 4 | financial reports |
| `mr-mantle/ledger/LedgerMaintenance` | 2.7 | 6 | merge into AccountingPeriodClose | same area as AccountingPeriodClose |
| `mr-mantle/ledger/ProductionCostClose` | 2.7 | 11 | merge into AccountingPeriodClose | same area as AccountingPeriodClose |
| `mr-mantle/ledger/ReceiptAdjustments` | 0.1 | 3 | merge into AccountingValuation | same area as AccountingValuation |
| `mr-mantle/marketing/ContactListSend` | 0.4 | 14 | keep, trim to 6 | marketing sends |
| `mr-mantle/order/AccountingReturns` | 4.2 | 15 | merge into ReturnToResponseBasicFlow | the accounting of returns, onto the return flow |
| `mr-mantle/order/OrderBulk` | 0.3 | 4 | merge into OrderLifecycle | same area as OrderLifecycle |
| `mr-mantle/order/OrderLifecycle` | 0.5 | 9 | keep, trim to 4 | order status beyond the flows |
| `mr-mantle/order/OrderProcureToPayBasicFlow` | 0.9 | 23 | keep | test/suite.js end-to-end flow: the backbone |
| `mr-mantle/order/OrderToCashBasicFlow` | 1.3 | 18 | keep | test/suite.js end-to-end flow: the backbone |
| `mr-mantle/order/OrderToCashTime` | 1.2 | 1 | keep | test/suite.js end-to-end flow: the backbone |
| `mr-mantle/order/ReturnToResponseBasicFlow` | 0.5 | 12 | keep | test/suite.js end-to-end flow: the backbone |
| `mr-mantle/order/Subscriptions` | 0.2 | 3 | merge into OrderLifecycle | same area as OrderLifecycle |
| `mr-mantle/other/AccountingBudgets` | 2.6 | 11 | keep, trim to 4 | budgets |
| `mr-mantle/other/AccountingTax` | 1.3 | 13 | keep, trim to 4 | tax |
| `mr-mantle/other/BudgetServices` | 0.2 | 7 | merge into AccountingBudgets | same area as AccountingBudgets |
| `mr-mantle/other/TaxStatementServices` | 0.2 | 5 | merge into AccountingTax | same area as AccountingTax |
| `mr-mantle/party/ChatRecords` | 4.0 | 9 | merge into InternalMessages | same area as InternalMessages |
| `mr-mantle/party/Consent` | 0.7 | 17 | merge into Contacts | same area as Contacts |
| `mr-mantle/party/ContactAddresses` | 0.5 | 4 | merge into Contacts | same area as Contacts |
| `mr-mantle/party/ContactBooks` | 2.7 | 20 | merge into Contacts | same area as Contacts |
| `mr-mantle/party/Contacts` | 1.3 | 12 | keep, trim to 4 | parties and contacts |
| `mr-mantle/party/ContentRecords` | 0.0 | 3 | merge into InternalMessages | same area as InternalMessages |
| `mr-mantle/party/CrmMiscServices` | 0.3 | 7 | merge into Contacts | same area as Contacts |
| `mr-mantle/party/InboxThreads` | 3.9 | 13 | merge into InternalMessages | same area as InternalMessages |
| `mr-mantle/party/InternalLabels` | 0.2 | 1 | merge into InternalMessages | same area as InternalMessages |
| `mr-mantle/party/InternalMessages` | 2.7 | 15 | keep, trim to 6 | messages, inbox, chat records |
| `mr-mantle/party/PartyAccounts` | 0.1 | 5 | merge into Contacts | same area as Contacts |
| `mr-mantle/party/PartyServices` | 0.1 | 8 | merge into Contacts | same area as Contacts |
| `mr-mantle/party/PartyTimePeriods` | 0.1 | 6 | merge into Contacts | same area as Contacts |
| `mr-mantle/party/RecordLinks` | 0.4 | 19 | merge into Contacts | same area as Contacts |
| `mr-mantle/party/RecordNotes` | 0.2 | 7 | merge into Contacts | same area as Contacts |
| `mr-mantle/party/RecordVisibility` | 1.9 | 11 | merge into Contacts | same area as Contacts |
| `mr-mantle/pass/PassAuthority` | 0.1 | 5 | keep, trim to 4 | the PASS authority mr-pass builds on |
| `mr-mantle/product/AssetReservationMisc` | 0.4 | 11 | merge into WarehouseCounts | same area as WarehouseCounts |
| `mr-mantle/product/AssetReservationMultipleThreads` | 0.9 | 1 | keep | test/suite.js end-to-end flow: the backbone |
| `mr-mantle/product/ProductCatalogChecks` | 0.6 | 8 | merge into ProductCatalogServices | same area as ProductCatalogServices |
| `mr-mantle/product/ProductCatalogCsv` | 0.0 | 7 | merge into ProductCatalogServices | same area as ProductCatalogServices |
| `mr-mantle/product/ProductCatalogImport` | 1.2 | 10 | merge into ProductCatalogServices | same area as ProductCatalogServices |
| `mr-mantle/product/ProductCatalogServices` | 0.2 | 10 | keep, trim to 4 | the catalog |
| `mr-mantle/product/ProductCatalogSettings` | 0.3 | 4 | merge into ProductCatalogServices | same area as ProductCatalogServices |
| `mr-mantle/product/ProductContents` | 0.6 | 10 | merge into ProductCatalogServices | same area as ProductCatalogServices |
| `mr-mantle/product/ProductEffectivePrices` | 0.1 | 4 | merge into ProductPriceLists | same area as ProductPriceLists |
| `mr-mantle/product/ProductFeaturePolicy` | 0.0 | 2 | merge into ProductCatalogServices | same area as ProductCatalogServices |
| `mr-mantle/product/ProductLabels` | 0.8 | 9 | merge into ProductCatalogServices | same area as ProductCatalogServices |
| `mr-mantle/product/ProductLifecycle` | 0.1 | 3 | merge into ProductCatalogServices | same area as ProductCatalogServices |
| `mr-mantle/product/ProductMethods` | 2.2 | 11 | merge into ProductStandardCosts | same area as ProductStandardCosts |
| `mr-mantle/product/ProductPackaging` | 0.6 | 9 | merge into ProductCatalogServices | same area as ProductCatalogServices |
| `mr-mantle/product/ProductPriceLists` | 0.5 | 10 | keep, trim to 4 | prices |
| `mr-mantle/product/ProductPricePolicy` | 0.2 | 5 | merge into ProductPriceLists | same area as ProductPriceLists |
| `mr-mantle/product/ProductStandardCosts` | 4.2 | 25 | keep, trim to 6 | standard costs and methods, 25 features down to 6 |
| `mr-mantle/product/ProductStockTotals` | 0.0 | 1 | merge into WarehouseCounts | same area as WarehouseCounts |
| `mr-mantle/product/ProductVariants` | 0.2 | 6 | merge into ProductCatalogServices | same area as ProductCatalogServices |
| `mr-mantle/product/Promotions` | 0.1 | 5 | merge into ProductPriceLists | same area as ProductPriceLists |
| `mr-mantle/product/ReportLookups` | 0.1 | 5 | merge into WarehouseCounts | same area as WarehouseCounts |
| `mr-mantle/product/SupplyPlanning` | 1.5 | 11 | merge into ProductionOrders | same area as ProductionOrders |
| `mr-mantle/product/WarehouseCounts` | 2.4 | 7 | keep, trim to 4 | stock: counts, moves, quality, expiry, traceability |
| `mr-mantle/product/WarehouseExpiry` | 0.3 | 6 | merge into WarehouseCounts | same area as WarehouseCounts |
| `mr-mantle/product/WarehouseMoves` | 1.5 | 10 | merge into WarehouseCounts | same area as WarehouseCounts |
| `mr-mantle/product/WarehouseQuality` | 2.6 | 7 | merge into WarehouseCounts | same area as WarehouseCounts |
| `mr-mantle/product/WarehouseReports` | 1.1 | 9 | merge into WarehouseCounts | same area as WarehouseCounts |
| `mr-mantle/product/WarehouseStockChecks` | 0.2 | 5 | merge into WarehouseCounts | same area as WarehouseCounts |
| `mr-mantle/product/WarehouseTraceability` | 1.0 | 11 | merge into WarehouseCounts | same area as WarehouseCounts |
| `mr-mantle/sales/AccountingStatements` | 1.3 | 10 | keep, trim to 4 | statements and sales reports |
| `mr-mantle/sales/SalesReports` | 0.1 | 4 | merge into AccountingStatements | same area as AccountingStatements |
| `mr-mantle/search/MantleSearch` | 0.4 | 8 | merge into UniversalSearch | same area as UniversalSearch |
| `mr-mantle/search/UniversalSearch` | 0.3 | 7 | keep, trim to 4 | search |
| `mr-mantle/shipment/CarrierServices` | 0.1 | 8 | merge into ShippoGateway | same area as ShippoGateway |
| `mr-mantle/shipment/ShipmentPick` | 0.2 | 6 | merge into WarehousePacking | same area as WarehousePacking |
| `mr-mantle/shipment/ShipmentReturnMisc` | 0.6 | 8 | merge into WarehousePacking | same area as WarehousePacking |
| `mr-mantle/shipment/WarehouseAdjustments` | 1.8 | 8 | merge into WarehouseReceiving | same area as WarehouseReceiving |
| `mr-mantle/shipment/WarehousePacking` | 3.5 | 9 | keep, trim to 4 | outbound |
| `mr-mantle/shipment/WarehousePicking` | 1.8 | 8 | merge into WarehousePacking | same area as WarehousePacking |
| `mr-mantle/shipment/WarehouseReceiving` | 3.6 | 12 | keep, trim to 4 | inbound |
| `mr-mantle/shipment/WarehouseReturns` | 1.9 | 5 | merge into WarehouseReceiving | same area as WarehouseReceiving |
| `mr-mantle/shipment/WarehouseShipping` | 2.9 | 10 | merge into WarehousePacking | same area as WarehousePacking |
| `mr-mantle/shipment/WarehouseTransfers` | 2.6 | 8 | merge into WarehousePacking | same area as WarehousePacking |
| `mr-mantle/work/CalendarChanges` | 2.1 | 10 | merge into CalendarServices | same area as CalendarServices |
| `mr-mantle/work/CalendarServices` | 2.1 | 20 | keep, trim to 6 | calendar, 20 features down to 6 |
| `mr-mantle/work/CommonsToday` | 1.6 | 14 | merge into CalendarServices | same area as CalendarServices |
| `mr-mantle/work/Dav` | 0.7 | 8 | merge into CalendarServices | same area as CalendarServices |
| `mr-mantle/work/ManufacturingServices` | 0.3 | 7 | merge into ProductionOrders | same area as ProductionOrders |
| `mr-mantle/work/MethodTemplates` | 0.7 | 4 | merge into ProductionOrders | same area as ProductionOrders |
| `mr-mantle/work/ProductionChanges` | 4.8 | 10 | merge into ProductionOrders | same area as ProductionOrders |
| `mr-mantle/work/ProductionCosts` | 1.1 | 4 | merge into ProductStandardCosts | same area as ProductStandardCosts |
| `mr-mantle/work/ProductionDowntime` | 1.4 | 7 | merge into ProductionShopFloor | same area as ProductionShopFloor |
| `mr-mantle/work/ProductionMaterials` | 2.2 | 5 | merge into ProductionOrders | same area as ProductionOrders |
| `mr-mantle/work/ProductionOrders` | 3.9 | 11 | keep, trim to 4 | production orders |
| `mr-mantle/work/ProductionPolicy` | 2.0 | 7 | merge into ProductionOrders | same area as ProductionOrders |
| `mr-mantle/work/ProductionQuality` | 3.7 | 8 | merge into ProductionShopFloor | same area as ProductionShopFloor |
| `mr-mantle/work/ProductionQuantities` | 4.8 | 9 | merge into ProductionOrders | same area as ProductionOrders |
| `mr-mantle/work/ProductionReports` | 1.3 | 6 | merge into ProductionOrders | same area as ProductionOrders |
| `mr-mantle/work/ProductionSchedule` | 1.2 | 7 | merge into ProductionOrders | same area as ProductionOrders |
| `mr-mantle/work/ProductionShopFloor` | 2.9 | 9 | keep, trim to 4 | shop floor |
| `mr-mantle/work/ProductionSubcontract` | 1.8 | 8 | merge into ProductionOrders | same area as ProductionOrders |
| `mr-mantle/work/ShipmentLoads` | 0.5 | 9 | merge into WarehousePacking | same area as WarehousePacking |
| `mr-mantle/work/SupportedAssembly` | 0.6 | 4 | merge into ProductionOrders | same area as ProductionOrders |
| `mr-mantle/work/TimeImports` | 0.1 | 7 | merge into WorkTimeEntries | same area as WorkTimeEntries |
| `mr-mantle/work/WarehouseProduction` | 2.4 | 8 | merge into ProductionOrders | same area as ProductionOrders |
| `mr-mantle/work/WorkCenters` | 1.6 | 6 | merge into ProductionShopFloor | same area as ProductionShopFloor |
| `mr-mantle/work/WorkGit` | 0.2 | 6 | merge into WorkProjectItems | same area as WorkProjectItems |
| `mr-mantle/work/WorkGitReview` | 0.5 | 10 | merge into WorkProjectItems | same area as WorkProjectItems |
| `mr-mantle/work/WorkMyTasks` | 0.1 | 1 | merge into WorkProjectItems | same area as WorkProjectItems |
| `mr-mantle/work/WorkPlanToCashBasicFlow` | 0.8 | 11 | keep | test/suite.js end-to-end flow: the backbone |
| `mr-mantle/work/WorkProjectBilling` | 0.6 | 7 | keep, trim to 4 | project billing |
| `mr-mantle/work/WorkProjectClients` | 1.3 | 12 | merge into WorkProjectBilling | same area as WorkProjectBilling |
| `mr-mantle/work/WorkProjectFoundation` | 0.4 | 5 | merge into WorkProjectItems | same area as WorkProjectItems |
| `mr-mantle/work/WorkProjectHub` | 0.5 | 7 | merge into WorkProjectItems | same area as WorkProjectItems |
| `mr-mantle/work/WorkProjectItems` | 0.6 | 10 | keep, trim to 4 | projects and work items |
| `mr-mantle/work/WorkProjectMilestones` | 0.6 | 10 | merge into WorkProjectItems | same area as WorkProjectItems |
| `mr-mantle/work/WorkProjectPlan` | 0.4 | 10 | merge into WorkProjectItems | same area as WorkProjectItems |
| `mr-mantle/work/WorkProjectReleases` | 0.3 | 7 | merge into WorkProjectItems | same area as WorkProjectItems |
| `mr-mantle/work/WorkProjectReports` | 0.7 | 8 | merge into WorkProjectItems | same area as WorkProjectItems |
| `mr-mantle/work/WorkProjectRequests` | 0.6 | 13 | merge into WorkProjectItems | same area as WorkProjectItems |
| `mr-mantle/work/WorkProjectServices` | 0.2 | 9 | merge into WorkProjectItems | same area as WorkProjectItems |
| `mr-mantle/work/WorkProjectSprints` | 0.3 | 6 | merge into WorkProjectItems | same area as WorkProjectItems |
| `mr-mantle/work/WorkResources` | 1.0 | 6 | merge into WorkProjectItems | same area as WorkProjectItems |
| `mr-mantle/work/WorkTimeEntries` | 1.5 | 8 | keep, trim to 4 | time |
| `mr-mantle/work/WorkTimesheets` | 1.9 | 8 | merge into WorkTimeEntries | same area as WorkTimeEntries |
| **Subtotal** | **210 s** | **1574** | **44 kept** | **projected 253 features, 35 s** |

## F. Small components

| Spec | Time | Features | Decision | Reason |
|---|--:|--:|---|---|
| `basic-comms/BasicComms` | 3.4 | 8 | keep, trim to 4 | BasicComms: drop the every-screen qvt render, keep its logic |
| `basic-comms/BasicCommsBridges` | 0.2 | 1 | merge into BasicComms | bridges |
| `basic-comms/BasicCommsCalls` | 0.4 | 8 | keep, trim to 4 | calls |
| `basic-comms/BasicCommsChat` | 0.2 | 3 | merge into BasicCommsEncrypted | chat |
| `basic-comms/BasicCommsComposeRef` | 0.2 | 7 | merge into BasicComms | compose retry |
| `basic-comms/BasicCommsEncrypted` | 0.1 | 7 | keep, trim to 4 | encrypted rooms |
| `basic-comms/BasicCommsFiles` | 0.1 | 5 | merge into BasicComms | the drive picker |
| `basic-comms/BasicCommsInbox` | 0.1 | 4 | merge into BasicComms | inbox |
| `basic-comms/BasicCommsTexts` | 0.1 | 2 | merge into BasicComms | texts |
| `dev-aide/ArtifactDocs` | 0.6 | 7 | merge into ArtifactIndex | Autodoc from the index |
| `dev-aide/ArtifactIndex` | 9.2 | 7 | keep, trim to 4 | the artifact index, 9 s: build it once for all features |
| `dev-aide/EntityModeler` | 0.1 | 7 | keep, trim to 4 | the entity modeler |
| `dev-aide/ServiceWriter` | 0.7 | 7 | merge into EntityModeler | the service writer |
| `example/Chat` | 0.1 | 4 | keep, trim to 4 | chat SQL on the real database |
| `example/Example` | 0.0 | 7 | keep, trim to 4 | the Rust service and tool, the example's point |
| `example/Files` | 0.1 | 4 | keep, trim to 4 | drives and files |
| `example/GitHosting` | 0.1 | 3 | merge into Files | git hosting |
| `example/MailClient` | 0.2 | 9 | merge into Chat | mail client SQL |
| `example/Wiki` | 0.2 | 5 | merge into Files | notes spaces |
| `framework/CallCaptions` | 0.0 | 5 | merge into Calls | captions |
| `framework/CallEncryption` | 0.2 | 7 | merge into Calls | encrypted calls |
| `framework/CallHandling` | 0.1 | 9 | merge into Calls | call handling |
| `framework/CallQueue` | 0.1 | 5 | merge into Calls | queues |
| `framework/CallRecording` | 0.2 | 11 | merge into Calls | recording |
| `framework/Calls` | 0.2 | 18 | keep, trim to 4 | calls: the core of the call services |
| `framework/ChatKeyBackup` | 0.2 | 6 | merge into ChatKeys | key backup |
| `framework/ChatKeys` | 1.3 | 11 | keep, trim to 4 | chat devices and keys |
| `framework/DatePeriod` | 1.0 | 6 | merge into ScreenHtml | date periods |
| `framework/EdgeVoice` | 0.1 | 6 | merge into Voice | SIP on an edge node |
| `framework/FieldTitle` | 0.0 | 6 | merge into ScreenHtml | field titles |
| `framework/Ivr` | 0.1 | 4 | merge into Voice | IVR |
| `framework/RecentAuth` | 0.1 | 2 | keep | recent sign-in |
| `framework/ResourceSearchExclusions` | 0.1 | 4 | keep | resource search exclusions, 4 features |
| `framework/RingGroup` | 0.1 | 8 | merge into Calls | ring groups |
| `framework/ScreenHtml` | 0.2 | 13 | keep, trim to 4 | html screen runtime |
| `framework/ScreenQvt` | 0.2 | 16 | keep, trim to 4 | qvt screen runtime |
| `framework/SipDevice` | 0.1 | 10 | merge into Voice | SIP devices |
| `framework/Transfer` | 0.2 | 9 | merge into Calls | transfer |
| `framework/Units` | 0.3 | 6 | merge into ScreenHtml | unit formatting |
| `framework/Voice` | 0.2 | 13 | keep, trim to 4 | voice |
| `framework/Voicemail` | 0.5 | 11 | merge into Voice | voicemail |
| `framework/WebPush` | 0.1 | 6 | merge into RecentAuth | user push subscriptions |
| `framework/XmppServices` | 0.0 | 7 | merge into ChatKeys | XMPP calls |
| `mr-marble/OrderRoleFilters` | 0.0 | 20 | keep, trim to 4 | FindOrder's role filters, no database |
| `mr-marble/ProjectSummarySource` | 0.0 | 1 | keep | one source check |
| `mr-marble/ScreenReport` | 0.0 | 1 | fixture | ignored unless asked: writes the screen report |
| `mr-pass/Account/PublicIdentity` | 0.1 | 6 | merge into Portals | public identity |
| `mr-pass/Account/SsoHandoff` | 0.0 | 2 | merge into Portals | SSO handoff |
| `mr-pass/Billing/Portals` | 0.3 | 3 | keep, trim to 4 | the portals through the PASS facade |
| `mr-pass/Forum/Conversations` | 1.1 | 11 | keep, trim to 4 | private conversations |
| `mr-pass/Forum/Forum` | 0.5 | 12 | keep, trim to 4 | the forum authority contract |
| `mr-pass/Forum/ForumAccess` | 0.9 | 15 | merge into Forum | access |
| `mr-pass/Forum/Import` | 0.6 | 7 | keep, trim to 4 | Discourse import |
| `mr-pass/Forum/InboundMail` | 0.4 | 11 | merge into Conversations | email participation |
| `mr-pass/Forum/Integrations` | 1.1 | 6 | merge into Forum | integrations |
| `mr-pass/Forum/Media` | 0.2 | 5 | merge into Forum | media |
| `mr-pass/Forum/Moderation` | 1.2 | 12 | keep, trim to 4 | moderation |
| `mr-pass/Forum/Operations` | 1.0 | 9 | merge into Import | retention runs |
| `mr-pass/Forum/Outbox` | 1.3 | 13 | merge into Forum | outbox |
| `mr-pass/Forum/Participation` | 0.9 | 11 | merge into Forum | participation |
| `mr-pass/Forum/Scale` | 0.0 | 1 | keep (slow) | opt-in large fixture: mark slow instead of an -D switch |
| `mr-pass/Members/Entitlements` | 1.1 | 9 | keep, trim to 4 | entitlements |
| `mr-pass/Transport` | 0.0 | 2 | keep | the authority's transport |
| `tools/ConfReload` | 0.2 | 6 | keep, trim to 4 | conf reload and components |
| `tools/DataView` | 1.4 | 1 | merge into MonitorSettings | Data View |
| `tools/MonitorSettings` | 0.1 | 2 | keep | monitor settings |
| **Subtotal** | **34 s** | **470** | **26 kept** | **projected 98 features, 10 s** |

## Duplicate renders

Screens rendered by more than one server spec in the same mode, from a static scan of `page()`, `render()`, `tui()` and
`qvt()` calls with a literal path (paths relative to each app's root screen; "default" is the helper's default mode,
qvt for the mr-boss SpecLibs' `page()`). Paths built in loops are not seen; those specs are in group A and go. The
keeper is the spec that survives the plan and owns the screen's logic: never the access spec when a logic spec also
renders it, and the spec named for the screen's first path segment when there are several.

| App | Screen | Mode | Rendered by | Render that keeps the checks |
|---|---|---|---|---|
| Accounting | `Approvals` | qvt | AccountingAccess, AccountingPayables | AccountingPayables |
| Accounting | `Banking/BankAccountDetail` | default | AccountingAccess, AccountingBanking | AccountingPayments |
| Accounting | `Banking/BankAccounts` | default | AccountingAccess, AccountingBanking | AccountingPayments |
| Accounting | `Payables/BillDetail` | default | AccountingBudgets, AccountingPayables | AccountingPayables |
| Accounting | `Payables/BillDetail/Match` | qvt | AccountingAccess, AccountingMatching | AccountingPayables |
| Accounting | `Payments/PaymentDetail` | default | AccountingPaymentGateway, AccountingPayments | AccountingPayments |
| Accounting | `Payments/PaymentDetail/Review` | default | AccountingPaymentGateway, AccountingPaymentRefunds | AccountingPayments |
| Accounting | `Payments/PaymentPrint` | default | AccountingPayments, AccountingScreens | AccountingPayments |
| Accounting | `PeriodClose/PeriodCloseDetail` | html | AccountingOutputs, AccountingPeriodClose | AccountingPeriodClose |
| Accounting | `Receivables/Collections` | default | AccountingAccess, AccountingCollections | AccountingReceivables |
| Accounting | `Receivables/InvoiceDetail` | html | AccountingAccess, AccountingReceivables | AccountingReceivables |
| Accounting | `Records/OrderDetail` | qvt | AccountingCollections, AccountingPayables | AccountingReceivables |
| Commons | `Files/DriveSettings` | qvt | Files, SyncSettings | Files |
| Commons | `Files/download` | qvt | FileShares, Files | Files |
| Commons | `Home` | qvt | CommonsSettings, Files, Notes | Tasks |
| Commons | `Settings/Calendars` | qvt | CalendarSettings, SyncSettings | Calendar |
| Commons | `Settings/ContactBooks` | qvt | ContactBookSettings, SyncSettings | Contacts |
| Commons | `Settings/Preferences` | qvt | CalendarSettings, CommonsSettings | Calendar |
| Comms | `Account` | default | MailAccounts, MailSignIn | MailAccounts |
| Comms | `Add/Connect` | default | MailAccounts, MailSignIn | MailAccounts |
| Comms | `Chatter/timeline` | default | Chat, Compose | Chat |
| Comms | `List` | default | MailAccounts, MailSignIn | MailAccounts |
| Manufacturing | `Approvals` | html | ManufacturingAccess, ManufacturingMethods | ManufacturingWorkOrders |
| Manufacturing | `Home` | html | ManufacturingMaterials, ManufacturingPlanning, ManufacturingReports | ManufacturingWorkOrders |
| Manufacturing | `Planning/Capacity` | html | ManufacturingAccess, ManufacturingReports, ManufacturingSchedule | ManufacturingPlanning |
| Manufacturing | `Planning/ProductionScheduleTui` | tui | ManufacturingAccess, ManufacturingSchedule | ManufacturingPlanning |
| Manufacturing | `Planning/SupplyPlanning` | default | ManufacturingPlanning, ManufacturingSubcontracting | ManufacturingPlanning |
| Manufacturing | `Planning/SupplyPlanning` | html | ManufacturingAccess, ManufacturingPlanning | ManufacturingPlanning |
| Manufacturing | `Planning/SupplyPlanning/convertSuggestions` | default | ManufacturingPlanning, ManufacturingSubcontracting | ManufacturingPlanning |
| Manufacturing | `ShopFloor/OperationExecution` | html | ManufacturingAccess, ManufacturingShopFloor | ManufacturingShopFloor |
| Manufacturing | `ShopFloor/ProductionTime` | html | ManufacturingAccess, ManufacturingShopFloor | ManufacturingShopFloor |
| Manufacturing | `ShopFloor/Station` | tui | ManufacturingAccess, ManufacturingShopFloor | ManufacturingShopFloor |
| Manufacturing | `WorkCenters/EquipmentDetail` | default | ManufacturingDowntime, ManufacturingWorkCenters | ManufacturingShopFloor |
| Manufacturing | `WorkCenters/FacilityDetail/WorkCenter` | default | ManufacturingSchedule, ManufacturingWorkCenters | ManufacturingPlanning |
| Manufacturing | `WorkOrders/Find` | html | ManufacturingAccess, ManufacturingWorkOrders | ManufacturingWorkOrders |
| Manufacturing | `WorkOrders/WorkOrderDetail` | html | ManufacturingAccess, ManufacturingWorkOrders | ManufacturingWorkOrders |
| Products | `Prices/PriceLists/PriceListDetail` | default | ProductsFinish, ProductsPricePrint | ProductsPricing |
| Projects | `Billing/InvoiceDetail` | default | Billing, BillingWalk, ClientBilling, ClientWalk, MilestoneBillingWalk | Billing |
| Projects | `Billing/InvoicePrint` | html-print | BillingWalk, ClientBilling, MilestoneBillingWalk | Billing |
| Projects | `Billing/Invoices` | default | Billing, BillingWalk, ClientBilling, ClientWalk | Billing |
| Projects | `Billing/Rates` | default | Billing, BillingWalk | Billing |
| Projects | `Billing/Unbilled` | default | Billing, BillingWalk, ClientBilling, MilestoneBillingWalk | Billing |
| Projects | `Billing/Unbilled/CreateInvoice` | default | BillingWalk, MilestoneBillingWalk | Billing |
| Projects | `Code/CommitDetail` | default | ClientCode, Code | Code |
| Projects | `Code/Compare` | default | ClientCode, Code | Code |
| Projects | `Code/MergeRequestDetail` | qvt | ClientCode, CodeReviewWalk, MergeRequests | Code |
| Projects | `Code/MergeRequestDetail/Changes` | default | ClientCode, MergeRequests | Code |
| Projects | `Code/MergeRequestDetail/ChangesTui` | default | ClientCode, MergeRequests | Code |
| Projects | `Code/MergeRequestDetail/Commits` | default | ClientCode, MergeRequests | Code |
| Projects | `Code/MergeRequestDetail/Overview` | default | ClientCode, CodeReviewWalk, MergeRequests | Code |
| Projects | `Code/MergeRequests` | default | ClientCode, MergeRequests | Code |
| Projects | `Code/Repositories` | default | ClientCode, Code | Code |
| Projects | `Code/RepositoryDetail` | qvt | ClientCode, Code | Code |
| Projects | `Code/RepositoryDetail/MergeRequests` | default | ClientCode, MergeRequests | Code |
| Projects | `Home` | default | Billing, ClientIsolation, ClientWalk, MergeRequests, Reports, Resources, TimeWalk | Billing |
| Projects | `Projects/MilestoneDetail` | default | ClientWalk, MilestoneBillingWalk, ReleaseWalk | Billing |
| Projects | `Projects/MilestoneDetail` | qvt | ClientIsolation, ClientWrites | ClientIsolation |
| Projects | `Projects/ProjectDetail` | qvt | ClientIsolation, ProjectDetail, ProjectsShell | WorkItems |
| Projects | `Projects/ProjectDetail/Code` | default | ClientCode, Code | Code |
| Projects | `Projects/ProjectDetail/Overview` | default | ClientIsolation, ClientWalk, ProjectDetail | WorkItems |
| Projects | `Projects/ProjectDetail/Team` | default | ProjectDetail, ResourceWalk, Resources | WorkItems |
| Projects | `Projects/ProjectDetail/Time` | default | BillingWalk, ClientBilling, TimeEntries | Billing |
| Projects | `Projects/Roadmap` | default | ClientIsolation, MilestoneBillingWalk, ReleaseWalk | Billing |
| Projects | `Requests/RequestDetail` | qvt | ClientIsolation, ProjectsShell, RequestWalk | WorkItems |
| Projects | `Requests/RequestDetail` | tui | ClientWalk, ProjectsTui | ClientIsolation |
| Projects | `Resources/Allocations` | default | ResourceWalk, Resources | TimeEntries |
| Projects | `Resources/Allocations/Staff` | default | ResourceWalk, Resources | TimeEntries |
| Projects | `Resources/TeamPlanner` | default | ResourceWalk, Resources | TimeEntries |
| Projects | `Time/TimeEntries` | default | ColumnChooser, Reports, TimeEntries | TimeEntries |
| Projects | `WorkItems/BacklogTui` | tui | ProjectsTui, SprintWalk | WorkItems |
| Projects | `WorkItems/List` | default | ClientAccessInternal, ClientIsolation | ClientIsolation |
| Projects | `WorkItems/WorkItemDetail` | default | CodeReviewWalk, MergeRequests, TimeEntries, TimeWalk, WorkItemDetail | WorkItems |
| Projects | `WorkItems/WorkItemDetail` | qvt | ClientIsolation, ProjectsShell | ClientIsolation |
| Projects | `apps/Commons/Tasks/TaskDetail` | default | MergeRequests, WorkItemDetail | Code |
| Warehouse | `Counts/Counts` | html | WarehouseAccess, WarehouseCounts | WarehouseInventory |
| Warehouse | `Handheld/Home` | html | WarehouseAccess, WarehouseMoves | WarehouseInventory |
| Warehouse | `Handheld/Move` | html | WarehouseAccess, WarehouseMoves | WarehouseInventory |
| Warehouse | `Home` | html | WarehouseAccessWalk, WarehouseMoves, WarehouseReports | WarehouseInventory |
| Warehouse | `Inspection/Inspections` | html | WarehouseAccess, WarehouseInspections | WarehouseReceiving |
| Warehouse | `Inventory/Adjustments` | html | WarehouseAccess, WarehouseAdjustments | WarehouseInventory |
| Warehouse | `Inventory/AssetDetail` | html | WarehouseAdjustments, WarehouseInventory, WarehouseIsolationWalk | WarehouseInventory |
| Warehouse | `Inventory/Assets` | html | WarehouseAccess, WarehouseInventory | WarehouseInventory |
| Warehouse | `Receiving/Putaway` | html | WarehouseAccess, WarehousePutaway | WarehouseReceiving |
| Warehouse | `Receiving/ReceiptDetail` | html | WarehouseAccess, WarehousePicking, WarehouseReceiving | WarehouseReceiving |
| Warehouse | `Receiving/Receipts` | html | WarehouseAccess, WarehouseReceiving | WarehouseReceiving |
| Warehouse | `Returns/Returns` | html | WarehouseAccess, WarehouseReturns | WarehouseReceiving |
| Warehouse | `Shipping/Pack` | html | WarehouseAccess, WarehousePacking | WarehouseShipping |
| Warehouse | `Shipping/ShipmentDetail` | html | WarehousePacking, WarehousePicking | WarehouseShipping |
| Warehouse | `Transfers/Move` | html | WarehouseAccess, WarehouseMoves | WarehouseInventory |
| Warehouse | `Transfers/Repack` | html | WarehouseAccess, WarehouseAdjustments | WarehouseInventory |
| Forum | `Account/Notifications` | default | Moderation, Outbox | Moderation |
| Forum | `Forum/Topics/TopicDetail` | default | Moderation, Participation | Forum |

## Method

The scripts are in the session scratchpad, not the repository; they are short enough to rewrite:

- `aggregate.py <log> <specs.txt>`: sums `Spec > feature  PASSED (1.23s)` lines per stage and maps each spec name to its
  file (`find runtime -name '*.spec.js'`), using the stage and the feature titles where two files share a name.
- `renders.py <specs.txt>`: the static render scan above.
- `plan.py <specs.tsv> <renders.tsv>`: the decisions, one per spec, and the tables and projections in this document.
