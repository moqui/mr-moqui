# MR Moqui application development practices

A guide for agents and people building business applications on MR Moqui: the IronStone BOSS apps (Projects, Products
and those that follow) over Mantle, and apps like them. It covers how the artifacts are organized and the practices
that keep an app's code and screens good. The reference guides for each tool (entities, services, screens, forms, the
facades) are separate; they are being ported from the Moqui Java documentation and extended.

Most of these practices come from reviews of the Products app (`runtime/component/mr-boss/plan/impl-products.md`,
section "R — Review" and findings F43–F46), of Accounting P0–P3 (`impl-accounting.md` "R — Review", F61 on) and of
Accounting P4–P5 (`impl-accounting.md` "R2 — Review"),
measured against the Projects app, which is the reference implementation of the screen archetypes. They cover code
quality, organization and UI quality, not individual bugs.

Contents: [1 tools and artifacts](#1-tools-and-artifacts) · [2 references](#2-references) ·
[3 where logic goes](#3-where-logic-goes) · [4 services and scripts](#4-services-and-scripts) ·
[5 screens](#5-screens) · [6 vue single-file screens](#6-vue-single-file-screens-qvue) · [7 tests](#7-tests) ·
[8 formatting and mechanical changes](#8-formatting-and-mechanical-changes) · [9 checklist](#9-checklist-before-calling-a-phase-done)

## 1. Tools and artifacts

### The layers

| Layer | Repository and directory | What belongs there |
| --- | --- | --- |
| Framework | mr-moqui: `crates/*`, `runtime/base-component/{framework,webroot,tools}`, `runtime/dev-component/{example,dev-aide,template}` | The facades, the screen renderers (`framework/template/screen/Screen*.js`), the browser client (`webroot/screen/webroot/js/WebrootVue.qvt*.js`), the XSDs (`framework/xsd`). No business data model. |
| Mantle | mr-mantle: `runtime/component/mr-mantle` | The business data model (entities, seed data) and the business services. It stands alone: no Mantle service, script, data file or spec names a BOSS group, service, screen or setting. It checks Moqui permissions itself (for example `PRODUCT_CATALOG_EDIT`, `PRICE_VIEW`). |
| Apps | mr-boss: `runtime/component/mr-boss` | The apps: screens, the app's access check, screen context (options, names, capabilities), settings forms, app-specific services. |

Each component is its own git repository. Commit in each repository that a change touches, with its own message.

### A component's directories

| Path | Contents |
| --- | --- |
| `component.xml`, `MoquiConf.xml` | The component and its conf: screen mounts (`screen-facade`), tool factories, properties. |
| `entity/*.xml` | Entity definitions, view entities and extensions (`extend-entity`). In BOSS these are rare; the data model is Mantle's. |
| `data/*.xml` | Seed, setup and demo data (`type="seed"`, `"seed-initial"`, `"demo"`): enumerations, status flows, security (`ArtifactGroup`, `ArtifactAuthz`, `UserGroupPermission`), demo records the walks use. |
| `service/<package path>/*.xml` | Service definitions: verb and noun, parameters, `<description>`, and the implementation, inline XML Actions in `<actions>` (§4). SECAs in `*.secas.xml`, EECAs in `entity/*.eecas.xml`. |
| `service/<package path>/*.js` | Script modules for logic too involved for XML Actions: one module per area (`Prices.js`, `Methods.js`), plus a shared library (`ProductsLib.js`). |
| `screen/<App>.xml`, `screen/<App>/**/*.xml` | XML Screens: the app root, its sections and subscreens. `screen/Shared/*.xml` holds screens mounted in several apps. |
| `screen/**/*.qvue` | Vue single-file screens and widgets for the browser (§6), each beside an XML screen. |
| `screen-extend/` | Extensions of another component's screens. |
| `template/*.xml` | Shared screen parts: `transition-include` files (`CatalogChoiceTransitions.xml`, `FilePickerTransitions.xml`), section includes; `*.fancy` text templates. |
| `test/*.spec.js` | Server specs, run by `moqui test` inside the server's script engine. |
| `test/client/*.spec.js`, `test/client/suite.js` | Client specs (HTTP, WebSocket, headless Chromium), run by `moqui test-client`; `suite.js` orders them and names the server specs that load their fixtures. |
| `plan/`, `docs/` | In mr-boss: research, design and implementation plans per app, the style guide; reference notes for built apps. |

### Languages and render modes

- **Services are XML Actions first.** A service's implementation is inline XML Actions in its definition; JavaScript is
  for logic XML Actions do not express well (§4).
- **Scripts are JavaScript** (QuickJS), not Groovy. XML action expressions, `<script>` bodies, conditions, `${}`
  template text and service scripts are all JavaScript. Groovy's `?:` and `?.` habits do not all carry over: use `||` or
  `??`. `moqui lint --groovy <dir>` reports Groovy left in a component; `moqui lint --screens` reports idioms and hazards
  in screen expressions.
- **A service script module** is a file whose last expression is its value, by convention `;({execute: run})`. A service
  calls it with `ec.resource.script(location).execute(op, context)`. `ec.resource.script` evaluates the file afresh
  in its own scope on each call, so module-level state does not survive between calls; per-call caches belong in the
  call (§4).
- **Screens render** in `qvt` (the browser: Vue 3 and Quasar), `tui` (the terminal, as a JSON node document; see
  [TUI](../ui/TUI.md) "For screen authors"), `html` and `html-print` (print and PDF). Every screen must work in `qvt` and
  `tui`. A `render-mode` text for one mode needs its twin for the other.
- **Decimals are not JavaScript numbers.** A `BigDecimal` from an entity or service supports `add`, `subtract`,
  `multiply`, `divide`, `compare`, `eq`, `isZero` and `toNumber`; `toDecimal(x)` makes one. String concatenation calls
  `valueOf` and throws: write `String(quantity)`.

### Commands

| Command | Use |
| --- | --- |
| `./moq run` | Start the server on the dev conf. |
| `./moq test <component> [--spec S] [--feature F]` | Server specs on a fresh database ([Testing](../Build-Test.md#component-specs)). |
| `./moq test-client <component> --serve -- --spec S` | Client specs against a started server ([Client-Testing](Client-Testing.md)). |
| `target/develop/moqui lint --screens [report-file]` | Screen expression lint. The argument is the report's **output** file, not a screen to check: passing a screen path overwrites that screen. |
| `moqui lint [service-file]`, `moqui lint --groovy <dir>` | Service expression lint; leftover Groovy. |
| `moqui tui`, `moqui tui --server` | The terminal UI, to see a screen's `tui` rendering. |

## 2. References

- **Style guide:** `runtime/component/mr-boss/plan/style-guide.md`. The shell (§2), the screen archetypes (§3: landing,
  list, detail, 360, task, board, wizard, report, settings and more), the widget guide (§4), color and status display
  (§5), states and feedback (§6), devices and breakpoints (§7), and the checklist for a new XML element (§8.1).
- **Archetype gallery:** `runtime/dev-component/example/screen/Gallery*.xml` and its `.qvue` files, at
  `/qapps/gallery` once signed in on a dev instance. One screen per archetype; its README lists them.
- **Reference implementation:** the Projects app, `runtime/component/mr-boss/screen/Projects/`. For record headers
  and actions see `Projects/Projects/ProjectDetail.xml` (the `m-record-header` block); for a landing see
  `Projects/Home.xml`.
- **Design documents:** `plan/design-doc-guide.md` (what a design contains), `plan/design-crosscut.md` (shared
  screens, cross-app contracts, posting rules), and the per-app `design-*.md`, `impl-*.md` and `research-*.md`.
  `plan/initial-design.md` holds the decided app set.
- **Products notes:** `runtime/component/mr-boss/docs/ProductsSharedScreens.md` (shared mounts and the shared
  context) and `ProductsImplementation.md`.
- **Framework guides in `docs/`:** [Services](../logic/Services.md) (errors, transactions, probe calls), [Testing](../Build-Test.md#component-specs),
  [Client-Testing](Client-Testing.md), [TUI](../ui/TUI.md), [Moqui-Java-Migration](Moqui-Java-Migration.md) (with the
  deliberate differences listed in [Parity-Diffs](../../plans/Parity-Diffs.md)), [Native-Components](Native-Components.md),
  [Web-Scripts](../interfaces/Web-Scripts.md).
- **Schemas:** `runtime/base-component/framework/xsd/xml-screen-3.xsd`, `xml-form-3.xsd`, `service-definition-3.xsd`,
  `entity-definition-3.xsd`. Their documentation elements describe MR Moqui's behavior where it differs.
- **Building:** [.agents/skills/building/SKILL.md](../../.agents/skills/building/SKILL.md), and the rules in
  [AGENTS.md](../../AGENTS.md) (failure tolerance, no Node dependency).

## 3. Where logic goes

- **Use Mantle before adding to it.** Mantle was ported from Moqui Java's mantle-usl, much of it used in production
  for years (`/moqui/runtime/component/mantle-usl` in a Java checkout is the original). Before writing a service, find
  the one that already does it and use it, or extend it with a parameter: payments are created, changed and posted
  through `create#`/`update#Payment` and `post#Payment` (so their SECAs run), applied through the `apply#*` services,
  refunded through `create#RefundPayment`; gateway operations go through the `*#SinglePayment` services and their
  gateway interfaces; invoices print over `get#InvoicePrintInfo` and send through an `EmailTemplate`. Accounting P4–P5
  first rebuilt each of these beside Mantle, with its own entities and guards, and the review removed the copies.
- **Business rules lean to Mantle.** A rule about the records (validation, effective dating, rounding, policy, totals)
  is a Mantle service in the package of the records it changes, with its own spec, passing under
  `./moq test mr-mantle` alone. The app keeps its access check, its screen context and its settings forms. Products'
  review moved catalog settings writers, bulk price changes, stock totals and cost subtotals from BOSS scripts into
  Mantle (`CatalogSettings.js`, `PriceChanges.js`, `get#ProductStockTotals`).
- **No pass-throughs.** A BOSS service that only renames a Mantle service adds nothing. A screen calls the Mantle
  service after the app's access check (for example in `always-actions` after `get#SharedContext`).
- **Check access in services, from permissions.** The check runs where the data is read or written, so it holds for
  imports, exports, search, shared mounts and direct requests. Derive capabilities from Moqui permissions
  (`ec.user.hasPermission`, `UserGroupPermission` rows in setup data), not from a map of BOSS group names, and not
  from `ec.user.isInGroup` in a service, the palette or a record link. Grant permissions to groups in data.
- **Reads come from view permissions.** Do not make an app's read capability "holds any of its permissions": another
  app's group is given an edit permission for its own work (Projects' managers hold `RECEIVABLE_EDIT` to bill), and it
  then opens this app's lists, search and links. Give each kind of record a view permission
  (`RECEIVABLE_VIEW`, `PAYABLE_VIEW`), let an edit permission alone open nothing, and spec what a neighboring app's
  group can reach.
- **Restricted values stay on the server.** A service leaves out of its answer what the user may not read (costs,
  supplier prices, margins); nothing is fetched and then hidden by the screen. Declare a service's out-parameters:
  declared ones are the only ones returned (none declared returns everything, internal helpers included).
- **Compare reason codes, not message text.** A policy that refuses returns a code (`denied(reason, message)` in
  `mantle/product/PricePolicy.js`), and callers test the code. Matching on an error's wording breaks when the wording
  changes. A new policy module starts with codes; Accounting's `AccountingPolicy.js` throws plain messages and is
  still to be converted.
- **Shared screens** (`screen/Shared/*.xml`) take `appRoot` and compute capabilities on the server through the app's
  shared-context service; the mounting app's context adds tabs and filters, never authority (design-crosscut §1.1).
  `sharedLinks` gives the paths to other shared screens under the current app.
- **One route per screen.** Do not keep hidden alias routes for an old path. When a screen moves, change the links and
  specs.

## 4. Services and scripts

### Definitions

- Give each service a `<description>`: what it does and what it refuses.
- Share parameter lists through an interface: a `type="interface"` service (`define#MethodParameters`) and
  `<implements service="..."/>` in each service that uses it. Products had service files of thousands of lines of
  copied parameter unions before this. An interface holds the in-parameters its services share; each service declares
  its own out-parameters. An interface that lists every service's outputs is the copied union again, and copying the
  interface into a second file to reuse it is a copy too: implement the first.
- Undeclared in-parameters are dropped before the service runs. A screen that passes `context` passes only what the
  service declares.

### Service implementations

- **Inline XML Actions** in the service definition: `entity-find`, `entity-find-one`, `entity-find-count`,
  `entity-make-value`, `entity-create`/`update`/`delete`, `service-call`, `set`, `if`, `iterate`, `return`, `message`.
  A Mantle service that checks its own permission and organization scope reads and writes with `disable-authz="true"`
  on these actions (and on `service-call`), as its script would with `.disableAuthz()`. A short JavaScript expression in
  a `set from` or a condition is fine.
- **A script module** only for logic that XML Actions do not express well: paging over several kinds of record,
  allocation planning, a posting preview, hashing, CSV, date arithmetic. Accounting P4–P5 first had nearly every
  service as a one-line `<script>` into a module of 1,000 to 1,600 lines, most of it entity and service calls.
- **No authorization by caller.** A service does not read `ec.artifactExecution.getStackNameString()` to decide whether
  its caller may write; it checks permissions. Status rules are `StatusFlowTransition` data that `update#` checks.
- **No EECA guarding an entity that only its own module writes**, and no EECA or SECA on a hot path (every `Payment` or
  `AcctgTrans` write, a framework entity, `service=".*"`) without a narrow `<condition>`.
- **Nothing for tests in production files.** Test entities, services, SECAs and scripts under `test/` are not loaded by
  `entity/` or `service/` files; a spec makes its own data and faults.
- Sequenced ids (`entity-sequenced-id-primary`, the `create#` services), not `ec.user.randomString`; status and kind
  fields are `StatusItem` or `Enumeration` ids with relationships, not free text.

### Script modules

- One module per area, with named functions and an explicit table of operations. An unknown operation throws:

  ```js
  const operations = {context, stock, projection, policy, storeFacility, storeTracking}
  if (!operations[op]) throw new Error("Unknown stock operation " + op)
  return operations[op](parameters)
  ```

- Put shared helpers in one library module (`ProductsLib.js`: `find`/`one`/`rows`/`rowsIn`, `ms`, `dateText`,
  `blankToNull`, `enumOptions`, `page`), not a copy in each file. Copies drift: Products had three versions of the
  empty-value check and two date conversions with different results.
- Per-call state (the app route, cached access, record reads) lives in an object made for the call
  (`lib.forCall(appRoot)`), not in module variables.
- Write code people read: real names for variables and functions, short lines. Products' first scripts averaged
  150–300 characters a line with one-letter names; the review rewrote them as named functions and ran Prettier (§8).
- Separate authorizing a write from building a screen's context. A write that builds the whole screen context to
  check access does every read twice.

### Queries

- Put conditions, ordering, `limit`/`offset` and `count()` in the entity find. Do not list a whole table and filter
  it in JavaScript.
- Read names and related rows for a list in one `in` query (`rowsIn`), not one `one()` per row. Check the visibility
  of many records in one call, not one call per row.
- Use `.selectField(...)` and `.distinct(true)` when only a few values are needed.
- Cap lists that could grow without bound (a dropdown of every product capped at 200 is not a dropdown; use a
  server-search choice, §5) and paginate long lists.
- **Search ignoring case, in the query, before the limit:**
  `makeCondition(field, "like", "%" + term + "%").ignoreCase()` on the record's names as well as its id. Filtering in
  JavaScript after `limit` finds nothing past the first page, and a plain `like` misses "Cash" for "cash".
- `makeCondition` takes three arguments for a value: `(field, "equals", value)`. With two, the second is read as the
  operator. And `not-equals "Y"` leaves out rows where the field is null; add an `is-null` alternative when null means
  "not Y".

### Failure tolerance and errors

- As [AGENTS.md](../../AGENTS.md) says: when one independent item fails (a row, a facility, a record of a list), log an
  error naming it and go on with the rest. Wrap each item in `try`/`catch`.
- A call whose failure the caller expects and gets past runs as a probe, in its own transaction, with
  `ec.message.pushErrors()`/`popErrors()` around it ([Services](../logic/Services.md) "A probe call"). Clearing the errors
  does not clear the transaction's rollback mark.
- Check concurrency: pass `lastUpdatedStamp` or the service's `expectedRevision` back on update so a stale write is
  refused.

### Values

- **No hard-coded defaults** for currency (`'USD'`), units (`'OTH_ea'`), languages (`'es'`) or purposes. Take them
  from the organization's settings, then the user's preferences, else leave the choice empty for the user.
- **Dates in the user's zone:** `ec.l10n.format(date, pattern)`, not `toISOString()` and a `UTC` label. This holds in
  specs too: a day computed with `toISOString().slice(0, 10)` is tomorrow's from evening on in the Americas, and a
  spec that locks "yesterday" that way fails every evening.
- **Display values come from the service.** A row for a screen carries its display text: dates as text in the user's
  zone (Accounting's `AccountingPolicy.display` adds a `<field>Text` for each date and stamp), names for ids, status
  names. JSON drops date types, and a screen that loads a script module to format dates evaluates the whole module on
  every render.
- **Amounts round to their currency** (`ec.l10n.roundCurrency`), not a fixed `setScale(6)`.
- **Changes by effective date:** a policy, price or setting change starts a dated row and ends the one in effect;
  history stays readable.

## 5. Screens

### Structure

- **Element order** follows the XSD: `parameter`, `always-actions`, `transition`/`transition-include`, `subscreens`,
  `pre-actions`, `condition`, `actions`, `widgets`, `fail-widgets`. In a form-list: the fields, then `columns`
  (including `columns type="mobile"`), then `view-board`.
- **`always-actions` stay small:** the shared-context check and what every subscreen needs. A subscreen rendered on
  its own (a standalone tab render, a TUI tab) runs its parents' `always-actions` but not their `actions`, so data a
  tab needs from its parent goes in `always-actions`; everything else goes in the tab's own `actions`. A screen path
  rendered with a trailing `/` renders the default subscreen's widgets, not the parent's.
- **A detail screen reads its record in `actions`, not `always-actions`.** `always-actions` also run for each request
  to a transition, so a read there runs on every drop-down keystroke of the screen's `choices`. Read it with
  `transaction="force-new" ignore-error="true"`, set `recordFound`, and show a "Not available" state that links back
  to the list (ProjectDetail), instead of failing the page.
- **Do not repeat the access check** in every transition and section. Check once in `always-actions` and read
  `sharedMountContext.capabilities` where a widget depends on a capability.
- **Shared transitions** go in a `transition-include` file (`template/CatalogChoiceTransitions.xml`), not copied into
  each screen. A `transition-include` copies **one** transition by its name: a template with several needs one include
  per transition, and a form whose `transition` is not on its own screen posts to nothing. Shared widgets go in a
  `section-include` of the same file (`template/AccountingParts.xml`: the organization picker, a list's find form,
  report options).
- **Screens that differ only by record kind share one template.** Accounting's invoice and bill screens were 5-line
  diffs of each other; now each route screen sets the kind in its actions and includes
  `template/AccountingDocuments.xml`'s sections. Look for such pairs in the diff before calling a phase done.
- **Declare only the parameters a screen reads.** A transition's response adds every declared parameter of its target
  that is in the context, so a copied block of parameters carries stale values to other screens (Accounting's
  delete of a draft went back to a list filtered by the deleted id). A response then needs no `parameter` list beyond
  what the target lacks.
- **A form's `map` falls back to the context.** A field missing or empty in the map takes the context's value, so an
  empty map does not keep a form blank, and an Edit form that `extends` an Add form in a list row reads the row's
  values. Do not keep a page variable named like a field of a form on the page unless the form should show it.
- **The screen context holds data, not functions.** A `set` of an arrow function fails when the context is
  converted; compute the values instead.
- **A transition with no actions is a link.** The form that targets it renders as a GET link form (`m-form-link`),
  which does not post a row-selection's checked rows. A row-selection action that opens another screen with the
  selection needs at least one action in its transition (Products' `bulkPrice` normalizes the id list).
- **Open a dialog after a reload** with the `_openDialog=<dialog id>` parameter, for "edit this row" links that reload
  the page with the row selected.
- **Links:** `target-window="_blank"`, not `target`. Other screens of the app through `sharedLinks`, not literal
  paths.

### The archetypes

Follow the style guide's archetype for each screen, as the Projects app does. In particular:

- **Record header** on detail and 360 screens (style-guide §3.3, §3.4), as in `Projects/Projects/ProjectDetail.xml`:
  `m-record-header` holding `m-record-title` (a key chip, an h5 name and a caption of facts),
  `m-record-status` (a `status-bar` or a state chip), `m-record-foot` (tags, links) and `m-record-actions`. Readers
  see the same header with fewer actions, not a separate label stack.
- **One primary action**, chosen by status (Approve, Publish, Activate; New when there is no record yet):
  `btn-type="primary"` on a link, `type="primary"` on a `container-dialog`. The other actions go in a `button-menu`
  "More" with `icon="more_horiz"`.
- **Ends and deletes** use `btn-type="danger"` and a `confirmation` that names the record. Activate, publish and generate
  confirm too.
- **Landing** screens use `number-card`s in a `dashboard`, each linking to a filtered list, shortcut buttons with one
  primary, and an empty state that offers the first steps (Create, Import).
- **Tabs** use `subscreens-panel type="tab"` (or `subscreens-menu type="tab"` with `subscreens-active` inside a tab)
  with an explicit `menu-index` on each item, in the design's order.
- **Do not wrap one button in a box.** A box whose body is a single dialog button belongs in the header's actions or
  in the `box-toolbar` of the box it acts on (Add component on the Draft changes box).
- **Hide empty boxes** with a section condition, and show an empty state where the user needs to know why there is
  nothing ("This product has no manufacturing method yet. Start one with New method.").
- **"Not available"** states link back to the list.

### Lists

- **Phone columns on every list** a phone sees: `columns type="mobile"` with two columns. The first holds the record
  link and its name; the second the amount or state and the row actions. Products' first pass gave many lists only an
  ID and a state, so phone users could not see a product's name or a price's amount.
- **The first column is the record link.**
- **Names, not IDs:** `display-entity` (`mantle.party.PartyDetail` with `text="PartyNameTemplate"`,
  `moqui.basic.Enumeration`, `moqui.basic.Uom`, `moqui.basic.StatusItem`), or a `*Name`/`*Text` the service returns.
- **Amounts** with `currency-unit-field` and `align="right"`; quantities with `display format="#,##0.###"` and
  `align="right"`.
- **States as chips:** a `render-mode` with a `qvt` text
  `<q-chip dense square class="q-ma-none m-tonal text-${color}">` and a `tui` text built with `JSON.stringify`. A `tui`
  text must be JSON produced by `JSON.stringify({t: "text", text: ...})`, never JSON with `${}` pasted into it.
- **Row actions in the row** (style-guide §4.4): inline `container-dialog`s for the common actions (Schedule, Correct,
  End), and a "More" menu for the rest; not a box of per-row forms below the list.
- **Bulk actions** are `row-selection` actions on the list, not a separate screen where the user types IDs.
- `paginate` long lists; `select-columns` on main lists.
- **Pagination comes from the service.** A list a service pages returns its `<list>PageIndex`, `PageSize`,
  `PageMaxIndex`, `PageRangeLow` and `PageRangeHigh` (`ProductsLib.page`, Accounting's `find#Records`). A short,
  bounded list (a preview's findings, an audit trail, a document's lines) is `paginate="false"`. Never set the page
  fields by hand to fake one page: Accounting P2 had sixty such lines.
- **A row action's dialog id carries the row's key** (`EditLine_${acctgTransEntrySeqId}`); a fixed id repeats on
  every row.

### Inputs

- **Choose records with server-search drop-downs:** `drop-down` with `dynamic-options transition="choices"
  server-search="true" min-length="0"`, through the shared choices transition, which also returns the label of the
  selected value. No text inputs for IDs, and no drop-downs that load every record. The choices service is a table of
  named sources, like an operations table: each finds its authorized rows, searches its fields in the query (§4), and
  reads a selected value outside the page on its own; an unknown choice throws. A spec calls every choice once with a
  selected value that is not in the page.
- **Closed lists** (languages, currencies, units, types) are drop-downs from the server's options.
- **Wording:** dialog titles and buttons in the user's words, saying what they do ("Edit category membership",
  "Review price changes"), not form names. Hints go in `tooltip`, not in the field title ("Through", not "Through
  (exclusive)"). Option labels are words, not code tokens (`financialType`). Titles in sentence case.
- **No implementation words in the UI:** "phase", "later", "scheduled", "placeholder". An area that is not built is
  left out of the menu, and its page says what it is not yet, in the user's terms.
- Group related fields with `field-layout`, `field-row` and `field-row-big`; a wide dialog uses two columns.

### Hygiene

- The XML declaration, the component's full license header (copy it from a neighboring file; a one-line "See
  LICENSE.md" is not it) and a final newline in every file; one element per line; 4-space indentation. A rewrite of a
  placeholder keeps its header and its section's `menu-image`.
- Lint every changed screen (`moqui lint --screens` with a report file) and keep its idiom count from growing.

## 6. Vue single-file screens (`.qvue`)

- Use one only when the XML widgets cannot express the screen (the variant builder's matrix, a board or calendar
  beyond `view-board`/`view-calendar`). Prefer extending a widget (style-guide §8.1) when several screens need it.
- A `.qvue` sits beside an XML screen that declares `render-modes` and `server-static`, and keeps its transitions,
  parameters and authorization in that XML. It has a `tui` twin: a `<Name>.tui.json` beside it (usually one `component` node
  registered by name, as the gallery's `Board.tui.json`), or a separate XML screen (`VariantBuilderTui.xml`).
- Use the `m-*` inputs and the shell's feedback (`moqui.notifyMessages`, a `$q.dialog` confirm) so it looks and
  behaves like the XML screens: one primary button at a time, a count in the confirmation of a bulk action, result
  boxes only when there are results.
- The gallery's `Board.qvue`, `Calendar.qvue` and `QuickTask.qvue` (`runtime/dev-component/example/screen/Gallery/`)
  are the patterns.

## 7. Tests

### Server specs (`test/*.spec.js`)

- Each expression statement **directly** in `then` or `expect` is a condition. A comparison inside an `if`, a loop or
  a helper function is not checked. Products' first shots specs had their page checks in `if` blocks, and those
  checks never ran (F46). Use `assert(condition, message)` for a nested check, or compute a problem string and compare
  it at the top level.
- Every new screen is in `AppStructure.spec.js` (it renders, with a text that is there in every data state) and in
  `TuiRender.spec.js`. A screen whose content depends on data needs a check text present whatever the data.
- **A widget that fails renders its error into the output**, not into `errorMessages`: a check of status 200 and no
  error messages passes a broken page. Check the output for `Error rendering` and `Exception in render`, and check
  a text the page shows. List rows are in the html render; a qvt render needs `withStaticData()` and the app root with
  `lastStandalone: "-2"`, and a chip that has only qvt and tui texts is in neither html nor print.
- Every new service with an access rule gets a spec with a user who has the permission and one who lacks it
  (Products' `ProductsAccess`). **The negative case must fail for the permission:** use the same parameters as a
  positive case that succeeds, or compare a reason code. A helper that counts any error as a refusal passes on a
  validation error (Accounting's `store#DraftEntry` without its fields was "refused" for everyone).
- A spec of a control's absence also checks its presence for a user who has it: Accounting checked that readers do
  not see "New batch", a button that was named "New journal batch".
- Spec helpers an app's specs share (sign in as a user, the demo record ids) go in one place, not a copy in each spec;
  four copies of Accounting's demo ids disagreed about an account.
- A spec line that begins with `(` or `[` continues the line before it (no semicolons): compute the value in `when`
  and compare it in `then`.
- A Mantle spec creates its own data and does not depend on BOSS.
- Run the specs of the screens and services you touched while working, and the whole component before a commit.
  Those runs leave out the slow specs (`slow: true`), and so does every command unless given `--slow`.
- **Keep a spec fast.** A spec or feature over about 15 seconds is slow and only for a special case: a specific
  scenario, checked when its result is needed, not coverage. One over 30 seconds is marked `slow: true`
  ([Slow tests](../Build-Test.md#slow-tests)), and an agent runs it only with the user's permission (AGENTS.md).

### Client specs (`test/client/*.spec.js`)

- **No screenshot specs.** A spec asserts what a page shows; it does not save images that no one compares or looks at.
  To look at a screen, take a screenshot while working (the browser tools, or `page.screenshot` in a spec run by hand)
  and do not keep it in the suite. A failed browser feature still writes its own `failed-*.png` for debugging.
  The specs of a phone layout check overflow at 390 px; a theme is checked only where a feature depends on it.
- Open the browser pages in `setupSpec`, one per user and width. Module variables are not shared between cases.
- **Walks** for each scenario in the implementation plan: a user with only the needed grants, every click a user
  makes, and an assertion of the result.
- Click what the user clicks. A menu item that holds a dialog or link is a `q-item` around a button: click the button
  (`.q-menu button, .q-menu a`), not the `q-item`, which does nothing (F43 was this test mistake). Scope a header
  menu to its own actions container: a 360 screen has more than one "More".
- Find rows by the record they show (a link with the record's id), not by position.
- Do not run client specs while editing the screens they cover: the run loads the spec files at its start and the
  screens as they change.
- A client failure that also fails at the commit before your change (for example a PDF without Chromium) is recorded
  as such, not "fixed" by changing the test.

## 8. Formatting and mechanical changes

- **JavaScript** (service scripts and specs) is formatted with Prettier, used as a one-off tool, not a dependency, with
  the repository's `.prettierrc.json`: `{"semi": false, "tabWidth": 4, "printWidth": 130, "bracketSpacing": false,
  "trailingComma": "none", "arrowParens": "avoid", "singleQuote": false}` with `--parser babel`. Prettier finds the
  file from the formatted file's directory; Prettier run with its defaults (width 80, `{ a }`, trailing commas) is the
  wrong style, as Accounting P1–P3's scripts and specs were. Spec conditions are found by syntax, so formatting a spec
  is safe. After formatting a service script, check that its module value is still the last expression.
- **XML:** only `XMLLINT_INDENT="    " xmllint --format`, then confirm with a canonical comparison (`xmllint --c14n` of
  both versions with blank text removed) that only whitespace changed before replacing the file. Another formatter
  writes `" />` and reorders attributes.
- **Scripted edits** assert that each old text is present exactly once before replacing it; a replacement that matched
  nothing looks like a passing change. Re-indent a moved block to its new depth.
- Commit per phase and per repository, with the tests run named in the record (`plan/impl-<app>.md`).

## 9. Checklist before calling a phase done

- Business rules in Mantle with Mantle specs; the app keeps access, context and settings; no pass-throughs.
- Access checked in services from permissions; restricted values left out of answers; out-parameters declared.
- Scripts: one module per area, named functions, the shared library, bounded queries, failure tolerance per item.
- No hard-coded currency, unit or language; dates in the user's zone; amounts rounded to their currency.
- Screens follow their archetype: record header, one primary, More menu, danger and confirmation on ends.
- Every list has phone columns with name and amount or state, names not IDs, amounts with currency, states as chips.
- Records chosen with server-search drop-downs; no typed IDs.
- Empty boxes hidden, empty states explained, user wording in titles and buttons.
- `tui` texts built with `JSON.stringify`; the screen renders in `tui`.
- Every detail screen has the record header and a not-available state; every ending, bulk or irreversible action
  confirms, naming the record or the count.
- Screens that differ by record kind share a template; each screen declares only the parameters it reads.
- Mantle's services used or extended, not rebuilt beside them; service implementations in inline XML Actions, script
  modules only for complex logic; no caller-name guards, no self-guarding EECAs, nothing for tests in production files.
- `./moq test` for the app and Mantle, `moqui lint --screens` with no new idiom sites, and the walks run.
- A mechanical pass over the changed files:
  - `isInGroup(` in a service or script;
  - `PageRangeHigh` set in a screen;
  - `toISOString` in a script or spec;
  - `getStackNameString`, `ec.user.randomString` for an id, `ec.context.putAll(ec.resource.script(`, and a
    `component://.../test/` location in a `service/` or `entity/` file;
  - `like` without `ignoreCase`, and a `limit` before a JavaScript filter;
  - a form whose `transition` is not on its screen;
  - more than one `type="primary"`/`btn-type="primary"` on a screen at a time;
  - `" />` and the one-line license header in XML, and a long `parameter-map` or expression that could be named once
    in `actions` (report options, a record's link parameters);
  - Prettier with the repository's config shows no change.
- The design and implementation records updated: what was built, the commits, the findings, and what is left for later.
  Commands in a record name no machine's paths (`/Applications/...`, `/tmp/...`): point to [Testing](Testing.md) for
  the Chromium setting, and to a report file of the reader's choosing.
