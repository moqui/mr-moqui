# MarbleERP → MrMarbleERP (2026-10-17)

> Archived 2026-10-16: the migration report, complete. The acceptance work still open (under [Limits](#limits)) has a row in [Remaining-Work](../Remaining-Work.md#known-open-items). The screen migration tutorial it followed is not in this repository.

Performed the ScreenMigrationTutorial in `runtime/component/MrMarbleERP`, cloned from `https://github.com/moqui/MarbleERP` at `dfbea90`. The fork is `https://github.com/moqui/MrMarbleERP`; the component name stays `MarbleERP` to preserve component URIs, authorization records and the `/apps/marble` mount. Unchanged upstream history was pushed first; baseline, mechanical conversion and hand ports are separate commits.

## Result

- Component lint: **30 → 3 → 0 parse failures**, **0 flagged sites** at completion. Frozen baseline: the component's `MIGRATION-INVENTORY.md`.
- Fresh all-data load: **8,359 records, 571 tables, zero DDL/data errors**, ECAs enabled. The initial pristine load was blocked by the Java-only `moqui-fop` dependency; it passed after removing that dependency, before changing expressions.
- Converter: **26 map literals + 1 Elvis expression**, two files. Hand port: list spread, inline fancy template, native organization chooser, stored search-link role conditions (not covered by lint).
- Full tests: **429/429 features, 46 specs** (344 Mantle + 67 SimpleScreens + 18 Marble), zero failed/ignored/skipped.
- Core tests: **940 passed, zero failed, one ignored**.
- Authenticated Chromium: HTML dashboard/customer/supplier/order/invoice/payment; organization selection/clearing; QVT dashboard/order; real SVG barcode and PDF with extracted order text. Zero uncaught JS exceptions and zero HTTP failures.

See `runtime/component/MrMarbleERP/MIGRATION-STATUS.md` and `test/README.md` for reproducible commands, evidence and acceptance limits. The component directory is ignored by this framework repository and has its own Git history.

## What the Mantle migrations actually did

Sources: [ConvertProcess](ConvertProcess.md), [MantleFlowTests](MantleFlowTests.md), [TestHarness](TestHarness.md), and the component histories/JavaScript specs. These were expression/runtime migrations, not a new business data model or a SQL schema rewrite.

### mr-mantle-udm: model, setup data and document expressions

UDM supplies Mantle's entity definitions, relationships, types, enumerations and seed/demo data. Most of that declarative XML was retained. Embedded Groovy still had to be migrated:

- The first converter pass rewrote 41 Elvis expressions. Hand ports fixed setup/localized-message expressions and data-document expressions, including map/list operations, decimal sums, and date/number conversion.
- `DataDocumentField.fieldPath` expressions are stored in data, not ordinary service actions. They needed explicit JavaScript ports (`map`/`reduce`, template strings, `toNumber` for search-document numeric output). Loading a data record successfully does not prove its stored expression will execute.
- Search feeds were checked by actually generating documents; the wiki manual-document caller needed a host implementation before its pending feed entries could drain.
- Entity IDs, field names and relationship structure stayed stable, so USL and screens continued to reference the same data model.

Local UDM checkout: `runtime/component/mantle-udm`, origin `moqui/mr-mantle-udm`, revision `50be8e3`. Its history explicitly separates `ff1ce64` (mechanical conversion), `677f542` (hand ports), then document-expression/search changes.

### mr-mantle-usl: business services, EECA/SECA and executable tests

USL supplies orders, inventory, shipments, invoicing, payments, accounting, projects, payroll and related business actions. The service interfaces and XML action structure were kept; embedded Groovy was replaced with plain JavaScript.

1. **Convert safe syntax first.** Map literals became objects; `context + [..]` became `Object.assign`; Elvis, casts, literal equality, quoted relationship names, supported strings and literal-list membership were converted where safe. The initial pass included 1,462 map rewrites, 85 context copies and 649 Elvis rewrites. Converter output and semantic hand ports were separate commits.
2. **Load in isolation with ECAs enabled.** Seed/demo service calls exposed the first semantic failures: fiscal periods, variants, accounting flags and asset totals. Errors named the independent data file/service, allowing the next failure to be fixed without losing the rest of the load. A successful load was only the first gate, not completion.
3. **Port runtime semantics by type.** Host decimals use `add`/`subtract`/`multiply`/`divide`/`compare`, never JS arithmetic or identity equality. Empty arrays require `.length`; host find lists must be copied before mutation. Maps use property access, arrays use `push`/`includes`, and date math works on numeric epoch millis. Service arguments are copied, so helpers that formerly mutated a caller's map return that map as an out-parameter. Java static/class calls were replaced by supported host APIs, not broad Java collection emulation. Drools/KIE was removed rather than ported.
4. **Port and run the business flows.** The Spock originals were kept as the oracle; executable JS specs now run through `moqui test` in the same engine as the services. Effective time, pinned sequences, data checks, uploaded files, REST/ScreenTest and parallel tests preserve the meaningful checks rather than accepting weaker output.
5. **Expand beyond the original flows.** Coverage grew from 233/762 services called to **751/762** using additional payroll, banking, reporting, manufacturing, lifecycle, shipment, content and other specs. The documented Mantle suite is **344 features in 43 specs**. The remaining services include network/deprecated multi-tenant/upstream-stub cases; coverage is not proof of every branch or production behavior.
6. **Fix genuine framework gaps with regressions.** Examples: after-EECA `originalValue`, field defaults, encrypted equality conditions, view auto-parameters, entity-value equality, condition value typing, user-context organization filters, copied-map/list tracking, entity-aware expansion and the screen/test APIs. Security-sensitive schema checks remained strict; independent data/feed failures were isolated and logged.

Local USL checkout: `runtime/component/mantle-usl`, origin `moqui/mr-mantle-usl`, revision `14d327b`. Current combined UDM/USL lint was rerun during this task: **106 files, 14,562 expression sites, zero parse failures and zero flagged sites**. Neither Mantle checkout was modified by this Marble port.

### Why Marble was much smaller

The dependency chain is **mr-mantle-udm → mr-mantle-usl → MrSimpleScreens → MrMarbleERP**. UDM provides the data, USL the executable business services, and SimpleScreens almost all ERP pages and print templates. Marble itself owns only two screens: the app shell and dashboard. Preserving component names (`mantle-udm`, `mantle-usl`, `SimpleScreens`, `MarbleERP`) keeps component URIs, mounted pages, service names and authorization artifacts connected despite the fork directory/repository names.

Marble still needed manual checks outside lint: its two stored `DataDocumentLink.linkCondition` expressions parsed only when searched, the FreeMarker chooser needed the fancy dialect, and its jQuery dropdown needed native browser behavior. That is why the final check combined loaded-data expression tests, populated renders, ordinary authorization and a real browser, rather than stopping at zero lint failures.

## Framework issue found by the browser

The actual `/apps/marble/Order/OrderDetail` failed in the ShipmentLinks include with `Maximum call stack size exceeded`, although socketless specs passed. Nested widget templates and service calls in the full application shell exceeded a nested engine's 256 KiB allowance. Raising only the outer budget did not fix it.

The narrowly scoped fix gives request workers an 8 MiB native stack and a shared 4 MiB script budget. Nested request engines get the remaining allowance rather than a fixed 256 KiB cap. Service/non-request budgets, the minimum-remaining guard and nesting limit are unchanged. A unit test checks that request recursion still becomes a script error rather than a native stack overflow. Full tests and the HTML/QVT/browser smoke passed after the fix. No existing unrelated framework or component edits were incorporated into this task's commits.

## Limits

This is validation against the current local MR-Moqui and already-in-progress MrSimpleScreens migration, not an older released framework or a clean-checkout certification of that dependency. Wiki fixtures, exhaustive roles/organization isolation and CSRF, every mounted QVT interaction, physical checks/scanners and FOP pagination parity remain acceptance work. The upstream unavailable Marketing dashboard item was not invented as part of the port. Repeated demo loads can encounter upstream immutable AssetDetail rows; the fresh load is the migration gate, not repeated writes onto an existing database.
