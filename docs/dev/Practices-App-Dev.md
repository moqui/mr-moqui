# MR Moqui application development practices

A guide for agents and people building business applications on MR Moqui: the IronStone BOSS apps (Projects, Products
and those that follow) over Mantle, and apps like them. It covers how the artifacts are organized and the practices
that keep an app's code and screens good. The reference guides for each tool (entities, services, screens, forms, the
facades) are separate; they are being ported from the Moqui Java documentation and extended.

Most of these practices come from reviews of the Products app (`runtime/component/mr-boss/plan/impl-products.md`,
section "R — Review" and findings F43–F46), measured against the Projects app, which is the reference implementation
of the screen archetypes. They cover code quality, organization and UI quality, not individual bugs.

Contents: [1 tools and artifacts](#1-tools-and-artifacts) · [2 references](#2-references) ·
[3 where logic goes](#3-where-logic-goes) · [4 services and scripts](#4-services-and-scripts) ·
[5 screens](#5-screens) · [6 vue single-file screens](#6-vue-single-file-screens-qvue) · [7 tests](#7-tests) ·
[8 formatting and mechanical changes](#8-formatting-and-mechanical-changes) · [9 checklist](#9-checklist-before-calling-a-phase-done)

## 1. Tools and artifacts

### The layers

| Layer | Repository and directory | What belongs there |
| --- | --- | --- |
| Framework | mr-moqui: `crates/*`, `runtime/base-component/{framework,webroot,tools,example}` | The facades, the screen renderers (`framework/template/screen/Screen*.js`), the browser client (`webroot/screen/webroot/js/WebrootVue.qvt*.js`), the XSDs (`framework/xsd`). No business data model. |
| Mantle | mr-mantle: `runtime/component/mr-mantle` | The business data model (entities, seed data) and the business services. It stands alone: no Mantle service, script, data file or spec names a BOSS group, service, screen or setting. It checks Moqui permissions itself (for example `PRODUCT_CATALOG_EDIT`, `PRICE_VIEW`). |
| Apps | mr-boss: `runtime/component/mr-boss` | The apps: screens, the app's access check, screen context (options, names, capabilities), settings forms, app-specific services. |

Each component is its own git repository. Commit in each repository that a change touches, with its own message.

### A component's directories

| Path | Contents |
| --- | --- |
| `component.xml`, `MoquiConf.xml` | The component and its conf: screen mounts (`screen-facade`), tool factories, properties. |
| `entity/*.xml` | Entity definitions, view entities and extensions (`extend-entity`). In BOSS these are rare; the data model is Mantle's. |
| `data/*.xml` | Seed, setup and demo data (`type="seed"`, `"seed-initial"`, `"demo"`): enumerations, status flows, security (`ArtifactGroup`, `ArtifactAuthz`, `UserGroupPermission`), demo records the walks and screenshots use. |
| `service/<package path>/*.xml` | Service definitions: verb and noun, parameters, `<description>`, the implementation (`<actions>` with XML actions or a `<script>`, or `type="script"` with a location). SECAs in `*.secas.xml`, EECAs in `entity/*.eecas.xml`. |
| `service/<package path>/*.js` | The JavaScript behind the services: one module per area (`Prices.js`, `Methods.js`), plus a shared library (`ProductsLib.js`). |
| `screen/<App>.xml`, `screen/<App>/**/*.xml` | XML Screens: the app root, its sections and subscreens. `screen/Shared/*.xml` holds screens mounted in several apps. |
| `screen/**/*.qvue` | Vue single-file screens and widgets for the browser (§6), each beside an XML screen. |
| `screen-extend/` | Extensions of another component's screens. |
| `template/*.xml` | Shared screen parts: `transition-include` files (`CatalogChoiceTransitions.xml`, `FilePickerTransitions.xml`), section includes; `*.fancy` text templates. |
| `test/*.spec.js` | Server specs, run by `moqui test` inside the server's script engine. |
| `test/client/*.spec.js`, `test/client/suite.js` | Client specs (HTTP, WebSocket, headless Chromium), run by `moqui test-client`; `suite.js` orders them and names the server specs that load their fixtures. |
| `plan/`, `docs/` | In mr-boss: research, design and implementation plans per app, the style guide; reference notes for built apps. |

### Languages and render modes

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
| `./moq test-client <component> --serve -- --spec S --out DIR` | Client specs against a started server; screenshots to `DIR` ([Client-Testing](Client-Testing.md)). |
| `target/develop/moqui lint --screens [report-file]` | Screen expression lint. The argument is the report's **output** file, not a screen to check: passing a screen path overwrites that screen. |
| `moqui lint [service-file]`, `moqui lint --groovy <dir>` | Service expression lint; leftover Groovy. |
| `moqui tui`, `moqui tui --server` | The terminal UI, to see a screen's `tui` rendering. |

## 2. References

- **Style guide:** `runtime/component/mr-boss/plan/style-guide.md`. The shell (§2), the screen archetypes (§3: landing,
  list, detail, 360, task, board, wizard, report, settings and more), the widget guide (§4), color and status display
  (§5), states and feedback (§6), devices and breakpoints (§7), and the checklist for a new XML element (§8.1).
- **Archetype gallery:** `runtime/base-component/example/screen/Gallery*.xml` and its `.qvue` files, at
  `/qapps/gallery` once signed in. One screen per archetype; its README lists them. `test/client/GalleryShots.spec.js`
  is the pattern for screenshot specs.
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

- **Business rules lean to Mantle.** A rule about the records (validation, effective dating, rounding, policy, totals)
  is a Mantle service in the package of the records it changes, with its own spec, passing under
  `./moq test mr-mantle` alone. The app keeps its access check, its screen context and its settings forms. Products'
  review moved catalog settings writers, bulk price changes, stock totals and cost subtotals from BOSS scripts into
  Mantle (`CatalogSettings.js`, `PriceChanges.js`, `get#ProductStockTotals`).
- **No pass-throughs.** A BOSS service that only renames a Mantle service adds nothing. A screen calls the Mantle
  service after the app's access check (for example in `always-actions` after `get#SharedContext`).
- **Check access in services, from permissions.** The check runs where the data is read or written, so it holds for
  imports, exports, search, shared mounts and direct requests. Derive capabilities from Moqui permissions
  (`ec.user.hasPermission`, `UserGroupPermission` rows in setup data), not from a map of BOSS group names. Grant
  permissions to groups in data.
- **Restricted values stay on the server.** A service leaves out of its answer what the user may not read (costs,
  supplier prices, margins); nothing is fetched and then hidden by the screen. Declare a service's out-parameters:
  declared ones are the only ones returned (none declared returns everything, internal helpers included).
- **Compare reason codes, not message text.** A policy that refuses returns a code (`denied(reason, message)`), and
  callers test the code. Matching on an error's wording breaks when the wording changes.
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
  copied parameter unions before this.
- Undeclared in-parameters are dropped before the service runs. A screen that passes `context` passes only what the
  service declares.

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
- **Dates in the user's zone:** `ec.l10n.format(date, pattern)`, not `toISOString()` and a `UTC` label.
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
- **Do not repeat the access check** in every transition and section. Check once in `always-actions` and read
  `sharedMountContext.capabilities` where a widget depends on a capability.
- **Shared transitions** go in a `transition-include` file (`template/CatalogChoiceTransitions.xml`), not copied into
  each screen.
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

### Inputs

- **Choose records with server-search drop-downs:** `drop-down` with `dynamic-options transition="choices"
  server-search="true" min-length="0"`, through the shared choices transition, which also returns the label of the
  selected value. No text inputs for IDs, and no drop-downs that load every record.
- **Closed lists** (languages, currencies, units, types) are drop-downs from the server's options.
- **Wording:** dialog titles and buttons in the user's words, saying what they do ("Edit category membership",
  "Review price changes"), not form names. Hints go in `tooltip`, not in the field title.
- Group related fields with `field-layout`, `field-row` and `field-row-big`; a wide dialog uses two columns.

### Hygiene

- The XML declaration, the component's license header and a final newline in every file; one element per line;
  4-space indentation.
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
- The gallery's `Board.qvue`, `Calendar.qvue` and `QuickTask.qvue` (`runtime/base-component/example/screen/Gallery/`)
  are the patterns.

## 7. Tests

### Server specs (`test/*.spec.js`)

- Each expression statement **directly** in `then` or `expect` is a condition. A comparison inside an `if`, a loop or
  a helper function is not checked. Products' first shots specs had their page checks in `if` blocks, and those
  checks never ran (F46). Use `assert(condition, message)` for a nested check, or compute a problem string and compare
  it at the top level.
- Every new screen is in `AppStructure.spec.js` (it renders, with a text that is there in every data state) and in
  `TuiRender.spec.js`. A screen whose content depends on data needs a check text present whatever the data.
- Every new service with an access rule gets a spec with a user who has the permission and one who lacks it
  (Products' `ProductsAccess`).
- A Mantle spec creates its own data and does not depend on BOSS.
- Run the specs of the screens and services you touched while working, and the whole component before a commit.

### Client specs (`test/client/*.spec.js`)

- **Screenshots** of each screen at 390, 1280 and 1440 px, light and dark, from one table-driven spec per app
  (`mr-boss/test/client/ProductsShots.spec.js`): a group names a user, a screenshot prefix and its pages; a page may
  wait for a text, open a dialog or row action, and check what it shows. **Look at the screenshots.** Shots that pass
  can still show a broken layout.
- Open the browser pages in `setupSpec`, one per user and width. Module variables are not shared between cases.
- **Walks** for each scenario in the implementation plan: a user with only the needed grants, every click a user
  makes, and a screenshot of the result.
- Click what the user clicks. A menu item that holds a dialog or link is a `q-item` around a button: click the button
  (`.q-menu button, .q-menu a`), not the `q-item`, which does nothing (F43 was this test mistake). Scope a header
  menu to its own actions container: a 360 screen has more than one "More".
- Find rows by the record they show (a link with the record's id), not by position.
- Do not run client specs while editing the screens they cover: the run loads the spec files at its start and the
  screens as they change.
- A client failure that also fails at the commit before your change (for example a PDF without Chromium) is recorded
  as such, not "fixed" by changing the test.

## 8. Formatting and mechanical changes

- **JavaScript** (service scripts and specs) is formatted with Prettier, used as a one-off tool, not a dependency:
  `{"semi": false, "tabWidth": 4, "printWidth": 130, "bracketSpacing": false, "trailingComma": "none",
  "arrowParens": "avoid", "singleQuote": false}` with `--parser babel`. Spec conditions are found by syntax, so
  formatting a spec is safe. After formatting a service script, check that its module value is still the last
  expression.
- **XML:** `XMLLINT_INDENT="    " xmllint --format`, then confirm with a canonical comparison (`xmllint --c14n` of both
  versions with blank text removed) that only whitespace changed before replacing the file.
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
- `./moq test` for the app and Mantle, `moqui lint --screens` with no new idiom sites, the shots and walks run, and
  the screenshots looked at.
- The design and implementation records updated: what was built, the commits, the findings, and what is left for later.
