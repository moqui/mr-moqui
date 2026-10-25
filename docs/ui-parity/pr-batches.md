# Frontend parity PR batches

Updated 2026-10-25. Scope: apps and qapps, with Java runtime as the reference. User validates and merges; this agent does not merge.

## Current batch

Three signed commits follow #109 (`fix/html-empty-display`). All three PRs are open and linked to this thread. Validate the current stack, then merge this batch in order:

| Order | PR | Change | Commit | Base |
|---|---|---|---|---|
| 38 | [#130](https://github.com/moqui/mr-moqui/pull/130) | Java Font Awesome sort carets in merged and static qapps | `be3c1f95` | `fix/html-empty-display` (#109) |
| 39 | [#131](https://github.com/moqui/mr-moqui/pull/131) | Full-width navigation, exports and bounded page buttons | `21313d92` | `fix/qapps-sort-carets` (#130) |
| 40 | [#132](https://github.com/moqui/mr-moqui/pull/132) | Desktop apps navbar tool order and Bootstrap metrics | `7c6264b8` | `fix/qapps-list-navigation` (#131) |

Validation for this batch:

- 23 strict QVT Rust rendering/precompile checks and one static rendering fixture pass.
- 98 focused JavaScript/browser/CSS checks pass (66 core/merged/static/QVT CSS, 20 HTML CSS, 12 HTML/display). The footer visibility test fails before the correction and passes afterward for list/board/calendar/tree in both renderers.
- Enterprise development build succeeds with Rust 1.95.0 and the isolated target cache. All three signatures verify; worktree is clean.
- Four fresh route comparisons (eight screenshots) have no capture, HTTP or console errors. Invoice-list row spans, sorting and Find Options were checked live.
- Java and Rust both pass live next-page, last-page disable and jump-to-page checks with six invoices at pageSize=1. Temporary pagination fixtures were removed from both databases before shutdown.
- Apps navbar geometry matches Java at desktop width, and the menu collapses and opens at 390px.
- Final Step 4 ran: Java and Rust shutdown verified, neither forcibly killed, but final schema/records/statistics are unequal. Overall verification fails. Approved differences SHA-256: `eae7e91a595e85f17ce618714ac216b915323469353c35caebbbdf5edd380d1e`.

| Route | Apps before → after | Qapps before → after |
|---|---|---|
| FindInvoice | 0.4708% → 0.4551% | 9.5875% → 9.3792% |
| EditInvoice with UI_PARITY_INVOICE | 0.4076% → 0.3932% | 13.1547% → 13.1547% |

Percentages are differing full-page pixels. The qapps edit form and shell still need work. Captures precede only the final restoration of static alternate-view footer hiding, which does not affect the captured list view. Images are externally hosted in PR descriptions and are not committed.

For user validation: check apps/qapps Invoice → Find Invoice and UI_PARITY_INVOICE. Confirm carets, navigation, export controls, filters and page selection; inspect apps navbar tools at desktop and mobile widths. The new batch depends on the existing 37 layers below and must follow #109.

Review handoff completed 2026-10-23: #130, #131 and #132 were rechecked live as OPEN and MERGEABLE at the commits above. The [interactive review crops](../.runs/batch-review.html) show before/after navigation, carets and navbar with red outlines and an expandable Java reference. All three views were rendered successfully without console messages. This handoff reused the recorded test/build results; no implementation changed or test rerun was needed.

## Existing stack awaiting user validation

All 37 PRs below were open on GitHub when checked on 2026-10-23 and registered to this thread. Merge from top to bottom in this table, then the current batch. Separate shell/layout PRs belong to another stack and are excluded.

| Order | PR | Title | Base branch |
|---|---|---|---|
| 1 | [#66](https://github.com/moqui/mr-moqui/pull/66) | fix(html): match form widget vertical spacing | `master` |
| 2 | [#67](https://github.com/moqui/mr-moqui/pull/67) | fix(entity): clear empty form fields to null | `fix/html-form-widget-spacing` |
| 3 | [#69](https://github.com/moqui/mr-moqui/pull/69) | fix(html): match list filter summary layout | `fix/entity-auto-empty-fields` |
| 4 | [#70](https://github.com/moqui/mr-moqui/pull/70) | fix(qapps): restore list text sizes | `fix/screen-list-parity` |
| 5 | [#71](https://github.com/moqui/mr-moqui/pull/71) | fix(html): restore quick saved finds | `fix/qapps-list-typography` |
| 6 | [#72](https://github.com/moqui/mr-moqui/pull/72) | fix(qapps): retain header-only filter summaries | `fix/html-list-navigation` |
| 7 | [#74](https://github.com/moqui/mr-moqui/pull/74) | fix(html): align inline list filters | `fix/qapps-filter-summary` |
| 8 | [#75](https://github.com/moqui/mr-moqui/pull/75) | fix(html): align desktop navbar menus | `fix/html-inline-list-filters` |
| 9 | [#76](https://github.com/moqui/mr-moqui/pull/76) | fix(html): restore flat typed panel headings | `fix/html-navbar-menus` |
| 10 | [#77](https://github.com/moqui/mr-moqui/pull/77) | fix(html): keep dashboard page shell full width | `fix/html-panel-tones` |
| 11 | [#78](https://github.com/moqui/mr-moqui/pull/78) | fix(html): restore address and card utilities | `fix/html-dashboard-scope` |
| 12 | [#79](https://github.com/moqui/mr-moqui/pull/79) | fix(html): restore normal list editor sizing | `fix/html-detail-utilities` |
| 13 | [#80](https://github.com/moqui/mr-moqui/pull/80) | fix(html): restore dashboard process map styles | `fix/html-row-controls` |
| 14 | [#81](https://github.com/moqui/mr-moqui/pull/81) | fix(html): match Java heading typography | `fix/html-dashboard-utilities` |
| 15 | [#82](https://github.com/moqui/mr-moqui/pull/82) | fix(html): size remote single-select controls | `fix/html-heading-typography` |
| 16 | [#83](https://github.com/moqui/mr-moqui/pull/83) | fix(html): align titled inline form rows | `fix/html-remote-selects` |
| 17 | [#84](https://github.com/moqui/mr-moqui/pull/84) | fix(html): restore badge metrics and contrast | `fix/html-titled-rows` |
| 18 | [#85](https://github.com/moqui/mr-moqui/pull/85) | fix(html): omit empty field-link placeholders | `fix/html-badge-metrics` |
| 19 | [#86](https://github.com/moqui/mr-moqui/pull/86) | fix(html): size inline remote selects from loaded options | `fix/html-empty-field-links` |
| 20 | [#87](https://github.com/moqui/mr-moqui/pull/87) | fix(html): honor centered search rows | `fix/html-inline-select-width` |
| 21 | [#88](https://github.com/moqui/mr-moqui/pull/88) | fix(html): match disabled link contrast | `fix/html-search-row-align` |
| 22 | [#89](https://github.com/moqui/mr-moqui/pull/89) | fix(html): preserve words in form labels | `fix/html-disabled-links` |
| 23 | [#90](https://github.com/moqui/mr-moqui/pull/90) | fix(html): match native radio option spacing | `fix/html-label-wrapping` |
| 24 | [#91](https://github.com/moqui/mr-moqui/pull/91) | fix(screen): expose clients before body widgets connect | `fix/html-radio-layout` |
| 25 | [#92](https://github.com/moqui/mr-moqui/pull/92) | fix(html): keep desktop table actions visible | `fix/screen-client-readiness` |
| 26 | [#93](https://github.com/moqui/mr-moqui/pull/93) | fix(html): keep display identifiers intact | `fix/html-wide-table-overflow` |
| 27 | [#94](https://github.com/moqui/mr-moqui/pull/94) | fix(db): retain filter-only subselect members | `fix/html-display-word-wrap` |
| 28 | [#96](https://github.com/moqui/mr-moqui/pull/96) | fix(entity): expose datasource configuration name | `fix/view-filter-only-subselect` |
| 29 | [#97](https://github.com/moqui/mr-moqui/pull/97) | fix(html): honor nested row column widths | `fix/entity-database-conf` |
| 30 | [#98](https://github.com/moqui/mr-moqui/pull/98) | fix(entity): retain executed query diagnostics | `fix/html-field-row-columns` |
| 31 | [#99](https://github.com/moqui/mr-moqui/pull/99) | test(screen): refresh rendering expectations | `fix/entity-query-text` |
| 32 | [#100](https://github.com/moqui/mr-moqui/pull/100) | fix(html): align untitled inline rows | `test/screen-rendering-expectations` |
| 33 | [#101](https://github.com/moqui/mr-moqui/pull/101) | fix(html): restore inline row label height | `fix/html-untitled-inline-rows` |
| 34 | [#106](https://github.com/moqui/mr-moqui/pull/106) | fix(html): fit empty inline search controls | `fix/html-inline-row-labels` |
| 35 | [#107](https://github.com/moqui/mr-moqui/pull/107) | fix(screen): resolve dynamic filter labels | `fix/html-inline-search-minimum` |
| 36 | [#108](https://github.com/moqui/mr-moqui/pull/108) | fix(web): serialize entity JSON as records | `fix/dynamic-filter-summary` |
| 37 | [#109](https://github.com/moqui/mr-moqui/pull/109) | fix(html): retain empty display text lines | `fix/web-entity-json` |

## Validation limits

Full visual parity and database equality remain unmet. Pixel percentages describe screenshots, not business workflow completion. No approved differences or comparison masks were changed.

The interrupted `.runs/pr-stack-tip` worktree and its patch backup remain intact. Overlapping qapps shell and field changes were excluded from this batch.

## Recovery evidence

- Baseline invoice: `.runs/2026-10-23_18-29-49.411390Z` (captures clean; Java shutdown markers missing, comparison blocked).
- Intermediate navigation: `.runs/recovered-nav-captures` (four clean captures; before navbar change).
- Serial baseline list: `.runs/2026-10-23_18-40-22.115649Z` (four clean captures; shutdown verified; real Step 4 completed, databases unequal).
- The overlapping run `.runs/2026-10-23_18-36-55.467796Z` failed before readiness and is excluded.
- Final batch held run: `.runs/2026-10-23_18-41-27.972254Z`; captures in `.runs/batch-final-captures`.

Use `JAVA_TOOL_OPTIONS=-Dlog4j.shutdownHookEnabled=false` for these probes so Moqui can finish its shutdown logging before Log4j closes. Run one Java probe at a time because the checkout's extracted WAR directory is shared.

## UI-only dialog and widget batch — 2026-10-24

| Order | PR | Change | Status |
|---|---|---|---|
| 1 | [#140](https://github.com/moqui/mr-moqui/pull/140) | Java dialog placement, grouped field gutters, backdrop behavior | Open; 18 checks pass; recorded first/last-name positions match; other differences remain |
| 2 | [#141](https://github.com/moqui/mr-moqui/pull/141) | Java Select2 dropdowns, dimensions, dependency events | Open; depends on #140; 19 checks pass; all six customer dropdown bounds match Java |
| 3 | [#142](https://github.com/moqui/mr-moqui/pull/142) | Java invalid/focused field colors | Open; depends on #141; CSS check passes; paired computed appearance matches |
| 4 | [#143](https://github.com/moqui/mr-moqui/pull/143) | Java calendar popup, date formats and native form events | Open; depends on #142; 20 checks pass; identical toolbar/day bounds and saved date verified on both |
| 5 | [#144](https://github.com/moqui/mr-moqui/pull/144) | Java From/Thru calendars and dialog focus | Open; depends on #143; 11 checks pass; both popups open at Java coordinates; vertical difference retained |
| 6 | [#145](https://github.com/moqui/mr-moqui/pull/145) | Restore chart CSV downloads | Open; depends on #144; 9 checks pass; actual browser download succeeds on both; reference data empty |
| 7 | [#146](https://github.com/moqui/mr-moqui/pull/146) | Java Order By Selectivity widget | Open; depends on #145; 20 checks pass; nine choices and same-coordinate selection verified; 2.15625px offset and missing Find retained |
| 8 | [#147](https://github.com/moqui/mr-moqui/pull/147) | Restore all Find Options fields and Java spacing | Open; depends on #146; 22 checks pass; 10 narrated steps; both submit and reload sorting successfully; full-screen parity fails |
| 9 | [#148](https://github.com/moqui/mr-moqui/pull/148) | Load initial qapps dependent dropdowns | Open; depends on #147; 37 checks pass; eight narrated steps prove customer create and saved-name reload on both; coordinate differences remain |
| 10 | [#149](https://github.com/moqui/mr-moqui/pull/149) | Match qapps grouped form margins and card padding | Open; depends on #148; 42 checks pass; eight narrated steps; name/email/Create horizontal bounds match; vertical offsets remain |
| 11 | [#150](https://github.com/moqui/mr-moqui/pull/150) | Match qapps floating labels and dense fields | Open; depends on #149; 24 checks pass; eight stable narrated steps prove saved names and resolved state labels; State wrapping and vertical differences remain |
| 12 | [#151](https://github.com/moqui/mr-moqui/pull/151) | Match qapps dialog headers | Open; depends on #150; 24 checks pass; eight narrated steps prove saved names; State wrap remains |
| 13 | [#152](https://github.com/moqui/mr-moqui/pull/152) | Match inline dropdown widths; prevent State wrapping | Open; depends on #151; 24 checks pass; eight narrated steps; all measured main form bounds match except phone x offsets; shared entry/Create clicks and saved names pass |
| 14 | [#153](https://github.com/moqui/mr-moqui/pull/153) | Preserve inline label spacing and unlabeled input padding | Open; depends on #152; 37 checks pass; all 13 named dialog controls have identical bounds; both saved-name reloads pass; Java shutdown-marker check fails |

Proof: `.runs/ui-narrated-20261024/dialog-fix/watch.html` and `.runs/ui-narrated-20261024/form-layout-fix/watch.html`. Every collected step has Piper narration and an actual paired browser screencast. The control sweep does not establish complete business workflow coverage. Do not mark the full UI task complete from this batch.

Dropdown evidence: `.runs/ui-narrated-20261024/select-geometry-fix/watch.html` (22 clips, each narrated). Exact combobox bounds match, but full screen parity fails; controller shutdown verification also failed and is retained in `.runs/2026-10-25_05-56-11.358269Z/shutdown.json`. No full workflow or fresh Rust build claimed.

Field color evidence: `.runs/ui-narrated-20261024/validity-style-fix/watch.html` (22 narrated clips). Both engine shutdown verifications passed; whole-screen parity remains failed.

UI persistence scenario: `.runs/ui-narrated-20261024/customer-persistence-proof/steps/00023/step.json` verifies creation plus browser reload of `UI / Proof9` on both Java and Rust. Clip: `videos/step-00023.mp4`. This is one apps scenario, not completion of the customer process document or qapps.

Calendar proof: `.runs/ui-narrated-20261024/calendar-layout-fix/steps/00024/step.json` records identical popup control bounds; step 25 selects day 26 at shared coordinates; step 27 verifies saved birth date `2026-10-26` after Update Person and reload on both engines. Both shutdown checks pass. Narrated videos are indexed by `watch.html` and `videos.json` as rendering drains. Earlier failed popup iterations remain retained. No full-screen or all-process parity claim.


Date range proof: `.runs/ui-narrated-20261024/date-range-hit-fix/watch.html` contains seven narrated steps. Steps 5/6 open both calendars without recovery clicks. Rust remains 2.15625 pixels lower due to preceding widgets. Both shutdown checks pass. Prior recorder double-toggle failures remain retained and explained in the PR.

CSV proof: `.runs/ui-narrated-20261024/report-csv-download/watch.html` contains four narrated steps. Both download `OperatingRevenueExpense.csv` (zero bytes, equal hashes). Download mechanism only; populated report accuracy and button bounds remain unverified/different.

Broad sweep interruption: `coverage-0` through `coverage-3` retained 2,437 completed step records before a provider interruption stopped the engines and narrators. Each run has `interruption.json`; none claims collection completion or graceful shutdown. Remaining narrated videos are rendering from retained actual browser frames. The execution ledger includes these partial observations and keeps workflow completion unasserted.

Order By proof: `.runs/ui-narrated-20261024/order-by-layout-fix/watch.html` contains 11 narrated paired clips. Both select Period Name (Asc) without recovery; both shutdown checks pass. Full parity fails; missing Rust Find button is next work.

Find Options proof: `.runs/ui-narrated-20261024/find-dialog-submit-verified/watch.html`, 10 narrated clips. Shared-coordinate sort and Find clicks need no recovery. Both reloaded lists retain `-fromDate,periodName`; both shutdown checks pass. Earlier URL-query verification failure is retained.
