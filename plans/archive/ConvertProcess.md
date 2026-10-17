# Converting a Groovy component: the Mantle trial

> Historical: this work later moved to the forks `moqui/mr-mantle-udm` and `moqui/mr-mantle-usl` (branch `master`), and the Spock flows are now JavaScript specs run by `moqui test` (see [MantleFlowTests](MantleFlowTests.md)). Branch names and the Rust flow tests below are as they were then.

This is how `mantle-udm` and `mantle-usl` were converted until all their data loads into MR-Moqui. The goal was every data type with entity ECA rules on. Services and screens that the load never reaches were not ported. The work happened on 2026-10-14, on the `mr-moqui-temp` branch of each repository in `runtime/component`. Each repository has two commits: the converter's output, then the hand ports on top, so the two can be reviewed separately.

## Result

- `moqui load types=all` with entity ECAs on exits 0. It loads 8040 records and creates 571 tables. It also runs the data-file service calls `initPartyAccountingConfiguration` (fiscal periods) and `createVariantProducts` (12 variants), and the entity ECA rules that set GL account flags and asset totals.
- The search feed indexes `MantleParty`, `MantleProduct`, `MantleFacility`, and `MantleInventoryAsset`. The 8 `MantleWikiPage` rows stayed pending in this trial, because the framework's wiki services were not ported. The third iteration below ports the wiki document's manual data service, and nothing stays pending.
- Not done:
  - 532 service expressions still fail `moqui lint` in 62 service files, and 52 `map + [..]` places are flagged by the converter.
  - `moqui lint --groovy` counts 2083 sites with a Groovy idiom: decimal literals 702, collection methods 676, `new` of a Java type 301, `*.` 174, `in` 143, and so on.
  - Those services run only when called. The load does not call them.

## Process

1. **Scratch runtime.** Make a runtime that links the components and keeps its database out of the real `runtime/db`. Put it under `/tmp`: a Postgres Unix socket path must be shorter than 108 bytes, and the scratchpad path is not.

   ```sh
   R=/tmp/mrm-rt; mkdir -p $R/component; cp -r runtime/conf $R/conf
   ln -s $PWD/runtime/base-component $R/base-component
   for c in mantle-udm mantle-usl ironstone-boss; do ln -s $PWD/runtime/component/$c $R/component/$c; done
   ```

2. **Mechanical conversion.** Run the converter, first without `--write` to see the report, then with it:

   ```sh
   moqui convert --groovy runtime/component/mantle-usl              # report only
   moqui convert --groovy --write runtime/component/mantle-usl
   ```

   - **mantle-udm:** 41 Elvis rewrites.
   - **mantle-usl:** 1462 map literals, 85 context-plus copies, 649 Elvis rewrites, 318 empty-list rewrites, and 52 `map + [..]` places left for review.
   - The converter is idempotent: running it again on converted files rewrites nothing that was already rewritten.

3. **Load, fix the first failure, repeat:**

   ```sh
   rm -rf /tmp/mrm-rt/db
   MOQUI_RUNTIME=/tmp/mrm-rt MOQUI_FRAMEWORK=$PWD/framework MOQUI_ACTIONS_DEBUG=1 moqui_log_level=info \
       moqui load types=all
   ```

   - The last `Loading entity data from` line names the file.
   - An action error names the service (`in service ...`). It also names the generated statement (`at generated line N: ...`), and with `MOQUI_ACTIONS_DEBUG`, the XML element before it (`after // <set field=...>`).
   - Errors left in the message facade by a data file are logged after that file (`Error messages loading entity data from ...`).
   - `ERROR [main]` lines are the load. `[moqui-worker-N]` lines are the search feed running after each commit.
   - To read the database after a load, `psql` is not bundled. Run `postgres --single` from `db/install/<version>/bin`; the libxml2 shim is in that install's `lib/`, which the binaries find through their RUNPATH, so no `LD_LIBRARY_PATH` is needed.

4. **Split the commits.** Back up the final files, reset the repository, run the converter, and commit that. Then restore the backup and commit the hand ports.

## What the converter does

These are rewrites whose meaning is the same in JavaScript, made only in expression places:

- A Groovy map literal becomes an object.
- `context + [..]` becomes `Object.assign({}, context, {..})`.
- Elvis `a ?: b` becomes `a || b`.
- EDI grammar files get a `.js` copy.
- String-literal `equals`, simple casts, `.contains(` on known lists and strings, and empty `ArrayList`/`HashMap` constructors (the second iteration, below).
- **New in this trial:** in a `condition` attribute, a name that is the `list` of an `entity-find` or `entity-find-related` in the same file becomes `(name?.length > 0)`, and `!name` becomes `!name?.length`.
  - An empty list is false in Groovy, but an object (always true) in JavaScript.
  - `?.length` works for an entity list, an array, and null.
  - Mantle has about 320 such conditions. Missing this produced the hardest failures: a branch taken on an empty find, then `list[0].x` on undefined, or an early `<return/>` with no result.

## Hand ports, by kind

Each kind below names where it came up. Port to plain JavaScript; do not add Java look-alike shims to the runtime.

- **Java collections.**
  - `new HashSet([x])` becomes `[x]`.
  - `set.addAll(other)` becomes a `for ... of` with `push`, or `list = list.concat(Array.from(other))`.
  - `list.contains(x)` becomes `includes`.
  - `List.add(x)` becomes `push`.
  - `new TreeSet(map.keySet())` becomes `Object.keys(map).sort()`.
  - `addToListInMap(k, v, map)` becomes `(map[k] ??= []).push(v)`.
  - A list or map read from the context and changed in place keeps the change (tested in `script/actions.rs`).
  - Seen in: `find#RootGlAccountClassEnum`, `find#ProductFeatures`, `create#VariantProducts`, and `get#ProductManualDocumentData`.
- **Equality.** `'DEBIT'.equals(x)` becomes `x === 'DEBIT'`.
- **Map plus.** `map + [k: v]` becomes `Object.assign({}, map, {k: v})`. The converter flags these and leaves them. An entity value spreads into the copy, since it is a proxy over its fields. Seen in: `init#PartyAccountingConfiguration`.
- **Casts.** `(x as int)` becomes `parseInt(x, 10)` or `Math.trunc(x)`, and `as String` becomes `String(x)`.
- **Decimals.** A `BigDecimal` field or parameter is a host decimal in scripts, and operators on it throw.
  - `(a ?: 0.0) + (b ?: 0.0)` becomes `toDecimal(a ?? 0).add(b ?? 0)`.
  - `a < 0` becomes `toDecimal(a ?? 0).compare(0) < 0`.
  - `-a` becomes `toDecimal(a).multiply(-1)`.
  - A decimal used as a JavaScript number goes through `toNumber()`.
  - Seen in: `update#AssetFromDetail` and `create#TimePeriod`.
- **Dates.**
  - A date, timestamp, or `ec.user.nowTimestamp` reaches a script as epoch-millis text; a `Date` field is UTC midnight. Text such as `yyyy-MM-dd` assigned back is converted on write and in conditions.
  - The date code in `create#TimePeriod` and `check#NextFiscalTimePeriods` uses small helpers, `dateOf`, `textOf`, `plusDays`, `plusMonths` (which clamps the day as `java.time` does), and `plusUnits` for `TF_day`/`TF_wk`/`TF_mon`/`TF_yr`. They work in UTC, so a step never crosses a day boundary.
  - `Calendar`, `java.time.*`, and `new java.sql.Date(x.time)` go away.
  - Compare dates through `dateOf(..)`, not as text: epoch-millis strings of different lengths compare wrongly as text.
  - A JavaScript `Date` does not survive `<set field>`: it converts to a host value. Keep `Date` objects in script locals.
- **Script bodies.**
  - `import` lines and Java types (`def`, `ExecutionContext ec = context.ec`) go away.
  - `def f(..)` becomes a function or arrow.
  - `for (X x in list)` becomes `for (const x of list)`.
  - `catch (Exception e)` becomes `catch (e)`.
  - `newValue.setAll(context)` becomes `setFields(context, true, null, null)`.
  - `ec.service.sync().name("create", "a.B")` becomes `name("create#a.B")`.
  - `EntityCondition.IN` and `conditionDate` become `ec.entity.conditionFactory.makeActionConditionDirect(..)` and `makeConditionDate(..)`, which are what compiled XML actions use.
  - Seen in: `create#VariantProduct`.
- **Data-file Groovy.** A `LocalizedMessage` template in `MantleUdmSetupData.xml` (`TimePeriodBaseNameTemplate`) used `padLeft`, `getYear()`, and a string regex split. It now uses `padStart`, `slice`, and a regex literal.

The second iteration turned the safe ones of these into converter rules. See below.

## Second iteration: more converter rules

These rules were added and run on `mantle-usl`, in its own commit on the `mr-moqui` branch:

- **`string-equals` (45):** `'lit'.equals(x)` becomes `'lit' === x`, and `chain.equals('lit')` becomes `chain === 'lit'`. Both are wrapped in parentheses unless they are already the whole of a parenthesized group, such as an `if (...)`.
  - A Groovy `x.equals(..)` on null throws, where `===` is false.
  - `.equals` with no string literal on either side is left alone.
- **`cast` (44):**
  - `x as String` becomes `x?.toString()`, which is null for null as in Groovy.
  - `x as BigDecimal` becomes `(x == null ? null : toDecimal(x))`, and `x as int` (or `Integer`, `long`, `Long`) becomes `(x == null ? null : Math.trunc(toDecimal(x).toNumber()))`. These repeat the operand, so only a name chain is rewritten.
  - Groovy's `as` binds more loosely than arithmetic and ordering (`a + b as String` casts the sum). So a cast is rewritten only when its operand begins after `(`, `,`, `[`, `{`, `?`, `:`, `=`, `==`, `!=`, `&&`, `||`, `;`, or `return`.
  - The rest are left for review. The first run of this rule cast only the `b` of `a + b as String`; the spot check of the diff caught it.
- **`list-includes` (0 in Mantle):** `.contains(` becomes `.includes(` on a string literal or a found list of the file. Mantle's `contains` calls are on sets and on strings held in variables, which this rule cannot see.
- **`empty-collection` (12):** `new ArrayList()` becomes `[]`, and `new HashMap()` or `new LinkedHashMap()` becomes `{}`, with or without generics.

The load is unchanged: all data, ECAs on, exit 0, 12 variants. Service expressions that fail to parse went from 532 to 503. Most of what remains are whole Groovy script blocks, each one failure. `moqui lint --groovy` idiom sites went from 2083 to 2050. The `equals` fixes do not show in either count: `'X'.equals(y)` parses as JavaScript and fails only when it runs.

What is left needs a decision, not more safe rewrites:

- Java collections used as collections: `new HashSet(..)` 90, `new TreeSet(..)` 42, `new ArrayList(..)` with contents, `.add(` 366, `.put(` 204, `.addAll(` 57, `.contains(` 86, and `*.` spreads 174.
- JavaScript arrays, JavaScript `Set`, and Java's `Set` have different method shapes:
  - `add`/`contains` on Java's `Set`; `add`/`has` and a `size` property on a JS `Set`; `push`/`includes` on an array.
  - So rewriting call sites one by one needs the type at each call.
- `plans/archive/Architecture-Proposal.md` says JDK types such as `BigDecimal` and `TreeSet` are small host helpers with the same call shape. Following it would mean script globals `HashSet`, `TreeSet`, `ArrayList`, and `HashMap` with Java's methods. The converter would then only strip generics, and `*.name` could become `.map((it) => it.name)`.
- That is a choice about the script environment, so it is noted here rather than made. The ports so far use plain JavaScript.

## Third iteration: the search callers

This iteration put the search and sales report callers on `SearchFacade`. It covers mantle-usl, mantle-udm, and the ironstone-boss screens, on the `mr-moqui` branches. Each repository has the converter's output and the hand ports in separate commits.

Two converter rules were added:

- **`quoted-property` (58 in mantle-usl, 22 in ironstone-boss):** `value.'Rel#entity'` becomes `value['Rel#entity']`, and `value?.'name'` becomes `value?.['name']`. An entity value reads a relationship name as the related value or list, and `get(name)` now does the same, as Java's `EntityValue.get` does.
- **`gstring` (53 in mantle-usl):** a double-quoted or triple-double-quoted Groovy string with `${..}` becomes a template literal. One with a bare `$name`, a double quote inside an interpolation, or a backtick is left for review.

Places the converter does not reach:

- **Document expressions.** A `DataDocumentField.fieldPath` in parentheses is an expression, and the Mantle documents had Groovy there: `?:`, closures, `.sum`, `*.`, `.time`, and `CollectionUtilities` calls. They are hand ported in mantle-udm.
  - A decimal field reaches the expression as a host decimal, so arithmetic goes through the new `toNumber(x)` global.
  - Groovy's `x ?: 1.0` treats 0 as false, so it becomes `toNumber(x) || 1`.
  - A `date` expression field may return epoch millis, the form dates take in scripts, and the document stores a timestamp.
- **FreeMarker templates.** The fancy dialect has `if`, `list`, and `assign`, and no macros. So the sales summary's stats panels are built as data in the screen actions, and the `.fancy` template loops over them. Its `${}` output is not escaped, so attribute values go through `WebUtilities.encodeHtml`.
- **Java-only values in the context.** A service future from `callFuture()` is not a context value, so QuickSearch calls the lookup synchronously.
- **Smaller idioms:** `{..}.get(key)` on a map literal becomes `[key]`, and `char.letter` becomes a regex test.

The org limit in the search services moved out of the query string. `search#DataDocuments` takes a `filter` map, which goes to `SearchRequest.filter`. It is a trusted constraint: it does not score, and the user's query cannot widen it.

## Framework changes the trial needed

Each of these is Java behavior the port lacked. All are in the framework commit for this trial.

- **Script environment:**
  - `EntityValue.cloneValue()`.
  - `ec.transaction.isTransactionInPlace()` and `isTransactionCacheActive()` (the queries only; scripts do not begin or commit).
  - `ec.logger` `info`, `error`, `debug`, and `trace`.
  - `isText()`, `isBinary()`, and `getContentType()` on resource references.
- **Error messages:**
  - A service body's error names the service.
  - An actions error shows the generated line, and with `MOQUI_ACTIONS_DEBUG` the element before it.
- **Entity ECA rules:**
  - Every field of the entity is a name in the rule's context, null when not set, so an unset field does not read a caller's variable of the same name. Java's rule context is the whole value.
  - `find one` with a full primary key drops the other equalities, `IS NULL` ones included, as Java's `oneInternal` does with nulls from `condition(context)`.
- **Conditions:**
  - A text value compared to a date, time, or number field is converted to the field's type before it is bound, as Java's condition does. Epoch-millis text is read as a moment.
  - A map condition such as `condition(context)` on a view entity uses the view's aliases.
- **Search feed and load:**
  - Each document type is delivered in its own transaction, so one type that fails (the wiki manual data service) rolls back and leaves pending only its own rows. Its message errors are cleared.
  - A load waits, up to 120 seconds, for the service workers (the feed) to finish before the database closes.

The third iteration (the search callers) needed:

- `elasticQueryAutoWildcard` and `toNumber` as script globals.
- `ec.artifactExecution.getFindFiltersForUser(entityName)`. It returns null when no filter applies, as Java does.
- `ec.user.context`, a map for the execution context's user. Entity filter maps and apply conditions see it, as Java pushes the user context to evaluate them.
- `ec.user.getPeriodRange(period, offset, date)`, Java's other form. The date may be epoch millis.
- `ec.l10n.format` reads a whole number with a date pattern as epoch millis.
- `filter-map-list` on an entity list. The filter runs on an array copy, and the name is set after it.
- Component `MoquiConf.xml` files merge between the default and runtime confs, as in Java. The tools component mounts `tools` and `system` under `apps` through its own.
- `org.moqui.impl.WikiServices.get#WikiPageManualDocumentData` runs: it reads the page text by its space's root location. The other wiki services are still not ported.
- `setScale(scale, roundingMode)` on decimals, as Java's `BigDecimal.setScale`. The mode is a `RoundingMode` name or the old `ROUND_*` number. The invoice and payment total services use it.
- Entity filter maps:
  - `_join`, `_list`, and `_comp` work as in Java's `makeCondition(Map)`.
  - A filter's map is evaluated only for a find on its entity, or on a view with it as a member. Before, one map that did not evaluate closed every find.
  - Mantle's `filterMap` values were Groovy map literals. They are JavaScript objects now, in `MantleSetupData.xml`.

The earlier load work (component entity directories, rules, and data; services by path; `fromDate` and secondary ids on nested records) is in `plans/archive/Implementation-Phases.md`, after the gate's note on loading `mantle-udm`.

## Fourth iteration: `in` lists, more casts, multi-line maps

Phase 0a of the next round. Rules added to `moqui convert --groovy`, run on `mantle-usl` (own commit on the `mr-moqui` branch):

- **`in-list` (143):** `x in [..]` becomes `[..].includes(x)`, so `!(x in [..])` becomes `!([..].includes(x))`. `x` is one postfix expression (name chain, calls, indexes, or a group) and the text before it must be a boundary: start, `(`, `,`, `[`, `{`, `?`, `:`, `=`, `==`, `!=`, `&&`, `||`, `return`. Groovy's `in` binds like `<`, tighter than `==` and `&&`. These are left: `!x in [..]` and `a + x in [..]`, which group differently; `for (x in [..])` and `for (String x in [..])`, which are loops; `x in someList`, whose type is unknown; and a list followed by `.`, `[`, or an operator. The result is a call, so it is safe in any position.
- **`cast`, more operand shapes (+8):**
  - `as String` now takes any postfix operand, such as `a.b(c)[0].d as String` or `x.movePointRight(2).longValue() as String`, and becomes `?.toString()`.
  - `as int` and `as BigDecimal` now take an index, or a parenthesized group with no call, string, assignment, or ternary: `(a || 60) as int`, `(itemTotal + discountTotal) as BigDecimal`, `list[0].qty as BigDecimal`. The operand is repeated for the null test, which is why a call is left alone.
  - The boundary test no longer counts `&gt;` and `&lt;` as a place an operand may start, since the XML text has them where a source file has `>`.
- **Multi-line and nested map literals:** the map rule already spans lines and nests; this iteration adds tests for both. What is still left are the 52 `map + [..]` places, which need the type of the left operand.

`moqui lint --groovy` on mantle-usl: parse failures 582 to 580, idiom sites 1959 to 1825. Whole `<script>` blocks that fail for another reason stay failures.

## Fifth iteration: the hand port of mantle-usl services (2026-10-15)

Every service file of mantle-usl is now hand ported. `moqui lint --groovy runtime/component/mantle-usl` reports 0 parse failures (582 before this iteration); the idiom sites left (about 430) are lint false positives such as decimal `.add(`, `new Date`, and `?.`. The data load is unchanged (8136 records, exit 0). Drools/KIE was removed from the component rather than ported (the `kie/` module, the `ProductPriceKS` registration and service, and the KIE call in `get#OrderShippingRateLocal`).

The work ran in phases, each worker owning disjoint files and committing by path: ECA and data files; small services; medium services; then the six large files (`OrderServices`, `ReturnServices`, `PaymentServices`, `InvoiceServices`, `ShipmentServices`, `AssetServices`). A framework worker added the host features the ports needed, and flow workers then ran the Spock flows (`plans/archive/MantleFlowTests.md`) and fixed what lint cannot see. Eight flows pass in Rust; `OrderTenantAccess` (data commented out), `RestApiTests` (the `s1/mantle` screen cases need a ScreenTest harness) and `MantleUslSuite` stay ignored.

What lint does not catch, found only by running (check these first in any further port):

- **Decimals.** A host decimal is always truthy and its `valueOf` throws: `!amount`, `x || 1.0`, `<`, `>`, `+`, and `!=` between decimals need `isZero()`, `compare`, `add`, and `compare(..) != 0`. Seven payment services had `!amount` that never fired.
- **Find results are read-only host lists.** They have `map`, `filter`, `find`, `some`, `reduce`, `sort` (on a copy) and index access, but no `push`, `add`, `first()`, or `each`. Copy with `Array.from` before changing one.
- **Empty arrays and objects are truthy.** Test `.length` or `Object.keys(..).length`; an `x in list` is a key test in JavaScript, so it must be `list.includes(x)`.
- **Maps and lists passed to a service are copies.** A callee that filled the caller's map now returns it as an out-parameter (`get#GlAccountClassReportInfo` and its `classInfoById`).
- **The converter does not reach** `in-map="[k:v]"` attribute map literals (11 sites), elvis in `<set from>` (7), and `&&` in XML text, which must be `&amp;&amp;` (two files were not well-formed XML and lint did not say so; run `xmllint --noout` on every touched file).
- **Dates** are epoch-millis text. Compare through `Number(..)` or `dateOf(..)`; the ports use UTC where Java used the JVM or user zone, so results differ only where the server zone is not UTC.
- **Behavior changes** where the Groovy was wrong are listed in the commit messages of each file; the main ones: `set#MissingGlAccountFlags` (`is-null`, not `is-not-null`), `<while>` on an empty object, `reject#OrderItem` still tests an undefined `checkCancelPart` (left as it was), and EFW2 still never sets `fileText`.

Framework changes this iteration: entity-auto updates return `oldStatusId` and `statusChanged` and check `StatusFlowTransition`; `<iterate>` with `<continue/>` no longer loops forever; multi-line SECA and EECA `<condition><expression>` text evaluates; delete ECA after-rules see the whole row; `entity-find-one` takes the first row of a partial key; `auto-parameters` in `<out-parameters>`; `${}` in `order-by` and `select-field`; script nesting limit 64; `implements` loads the implemented file on demand; and the script surface listed in `moqui-host.d.ts` (`ec.context.sharedMap`, `sequencedIdPrimary`, `makeNotificationMessage`, decimal `divide(x, scale, mode)`/`abs`/`negate`/`scale`, `nextCronExecution`, min and max aggregations, REST `bytes()`, and others).

Known open items: `asset_reservation_multiple_threads` can fail with a Postgres deadlock in `set#OrderBillingShippingInfo` when it runs after other flows (it passes alone); `moqui lint --screens` fails on 18 expressions in ironstone-boss screens, which this iteration did not touch; services the flows do not reach (payroll, EFW2, Nacha, PositivePay, BAI, OFX, budget and financial-ratio reports, most of `work/`) are ported but have never run.

## Sixth iteration: lint that tells valid JavaScript from Groovy, and running the ports that no flow reaches (2026-10-16)

`moqui lint --groovy` on mr-mantle-usl had reported 434 flagged sites since the fifth iteration. They were nearly all valid JavaScript: `new Date`/`Set`, `.add(` on a host decimal, `.setScale(n, 'MODE')`, `catch (e)`, `ResourceReference.isTextFilename`, and the words `as` and `..` and numbers in comments. The lint now sorts each idiom into `Groovy`, `hazard` or `valid JavaScript`; only the first two are flagged (`--all` flags the third too), comments and template text are masked before matching, and the report totals are grouped by that severity. mr-mantle-usl is at 0 flagged sites, 0 parse failures.

New hazard rules (`script/lint_hazards.rs`, found by running the ports, then written as rules so the next port gets them for free): a host decimal under a comparison, arithmetic operator, `!x`, `x ||` or `?:`; a mutation (`push`, `first()`, `each(`) of an `entity-find` list; `x in array`; truthiness of a `List`/`Set` parameter or an `entity-find` list (an empty array is true in JavaScript); a `const`/`let` declared twice at the function level of one service (the generated script is one function, so the second `<script>` fails to run); and a regular expression given to `replaceAll`/`replace`/`split`/`matches` as a string (JavaScript reads it as text).

`moqui test --coverage` / `--coverage-report file` counts the services each spec calls (`service::coverage`, recorded in `sync_with`) and lists those it never called, per service file. mr-mantle-usl went from 233 of 762 services called (the 8 flows) to 459 with 13 new specs (HumanRes, Payroll, ContactAndFacility, Party, BankFile, BankOutput, FinancialReports, ProductCatalog, Budget, TaxStatement, InvoicePaymentMisc, WorkProject, LedgerMaintenance).

Bugs the new specs found, all in the ports or the host (the lint could not see them):

- Host: `ec.resource.expand(text, "", entityValue)` ignored an entity value as the context, so `PartyNameTemplate` and `TimePeriodNameTemplate` expanded blank; the `ignore="expr"` attribute of `econdition` and `date-filter` was passed as text and never true; `entry = list[i]; entry.field = x` changed a copy, not the list element, because a list or map put under a name was converted at once (now it stays the script's object until the next flush); a distinct select did not select its order-by fields (`find#Party` failed on Postgres); `EntityValue.deleteWithRelated` did not exist; a `${}` expansion in a `value=` attribute cannot call a function a `<script>` declared (use a template literal in `from=`).
- Ports: `list ? list[0] : null` on a find list (20 sites, an empty list is true); `result.contactMechId = ...` for an out-parameter in three facility services; a `const addDec` declared twice in `get#GlAccountClassReportInfo`; `replaceAll('[^a-z0-9-]+', '')` in `format#UrlSlug`; `subTimePeriod` passed for `timePeriod` in `run#BudgetExpenseDetailReport`; the credit card `expireDate` default `"/"` failing validation in `create#PersonCustomer`; a null `EmplPosition` in `create#EmploymentSalary`; a find list `concat` in `delete#Employment`; date relational compares that now go through `Number(..)`.
- Stubs that are stubs upstream too (a TODO body in the Java original), so a spec only checks they run: `get#VariantProductsInfo`, `autoApply#Payment`, `autoApplyTo#Invoice`, `calculate#SalesTaxLocal`.

## Seventh iteration: more specs, and the host gaps they found (2026-10-16)

The coverage went from 459 to 751 of 762 services called (344 features in 43 specs) with the specs in [MantleFlowTests](MantleFlowTests.md). The 11 services left need a network, belong to the deprecated multi-tenant mode, or are upstream stubs; [Remaining-Work](../Remaining-Work.md) names them. `moqui lint --groovy` is still at 0 flagged sites over 83 files.

Host bugs the specs found and fixed (each has a unit test):

- Auto-parameters of a view-entity were empty (they read `entity.fields`, a view has aliases): `create#PayrollAdjustmentAndDetail` returned no `payrollAdjustmentId`.
- An after entity ECA rule did not get `originalValue` (only before rules did). `handle#OrderPartChange` and `handle#OrderItemChange` use it to tell a part total change from a real one, so every `update#OrderPartTotal` ran `handle#OrderMajorChange`, which deleted the promotion item just created and then failed in `checkReserve#AssetForOrderItem` with "Order Item not found". The write now reads the row before it runs when an after rule has `get-original-value`.
- A find condition on an `encrypt="true"` field compared plain text with the stored ciphertext (the encryption has a constant IV so that a query by value works, as in Java), so `store#Order` never found a party by `PartyIdentification.idValue`. Equality and membership conditions now encrypt the value.
- `ec.service.sync().parameters(entityValue)` ignored an entity value (the bulk order store deletes the items an update dropped with `in-map="itemMissing"`).
- `ec.resource.expand(null, '')` threw; Java returns an empty string.
- `==` and `!=` between two entity values is identity in JavaScript (a Groovy `==` compares the fields): six services never saw an unchanged address, phone number, contact mech, credit card or bank account and always created a new row. `EntityValue.equals(other)` compares the entity and fields, the services use it, and the lint reports `==` and `!=` on a `cloneValue()` copy as `entity-value-identity`.

- Entity field defaults `ec.user.nowTimestamp` and `ec.user.userId` (41 fields in the mantle entities) were never applied, only literals and field names were, and an entity-auto create with a primary key that has a default (`contentDate` of `AssetMaintenanceContent`) sequenced the key to `"01"` and then collided on the second row. They are applied, the primary key default first.
- A test helper `uploadedFile(name, text, contentType)` makes the `FileItem` that the `save#...ContentFile`, `upload#...` and `update#FacilityCertification` services take, so those run in specs.
- The lint now reports a Java class written with its package (`org.moqui.resource.ResourceReference.isTextFilename(..)`, `org.moqui.impl.service.ScheduledJobRunner.getCronDescription(..)`): the script has no `org` name, so seven `save#...ContentFile` services and `get#OrderDisplayInfo` for a recurring order failed at run time.

Port bugs: the `calculate#PayrollAdjustment` output did not declare `payrollPhaseEnumId` and `garnishMinWageApplies`, so the garnishment limit (a share of disposable pay) never applied; `create#WorkersCompRate` did not pass the new adjustment id to its detail; `generate#Efw2File` looked up an agent party that was not given; seven services kept the upload stream in `fileStream = contentFile.getInputStream()` with no `const`, so a bare assignment went through the context stack and the stream was no longer a stream; `fulfill#ProductSubscriptionResource` and `init#Tenant` logged `ec.transaction.getTransactionManager()` (Java only); `create#ShipmentLoadsForOrders` never listed the picklists it created.

Differences kept, because Java behaves the same: a shipment put on a picklist by `create#ShipmentLoadShip` has no bin number; `add#OrderPartToShipmentLoad` on a part already on the load moves it to the next free bin; a note is keyed by its date, so two `store#Order` revisions in the same millisecond collide (only seen when a spec freezes the time).

