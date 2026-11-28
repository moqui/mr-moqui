# IronCalc DB 2: finishing CalcGrid for the first release

Status 2026-11-27: plan, not built. It follows [IronCalc-DB](archive/IronCalc-DB.md), built Nov 13, 2026 (Phases 0
to 4) and archived the same day as this plan was written. The IronCalc fork was merged with upstream on 2026-11-27
(`a0e66946`, [Working with upstream](archive/IronCalc-DB.md#working-with-upstream)). This plan takes the items that
plan left open and that the user chose to finish on Nov 27, 2026. What waits on IronCalc upstream, the larger engine
changes, the WASM engine in the browser and the three items moved out at review are in
[Future-Plans](Future-Plans.md#ironcalc-and-calcgrid). The guides are [Calc mode](../docs/tools/Database.md#calc-mode),
[CalcGrid and access profiles](../docs/tools/Database.md#calcgrid-and-access-profiles) and
[CalcGrid rooms](../docs/ui/Qvt-Client.md#calcgrid-rooms).

Paths: `calc/` is `crates/moqui-calc/src/`, `tui/` is `crates/moqui-tui/src/`, `report/` is `crates/moqui-report/src/`,
`js/` is `runtime/components/base/webroot/screen/webroot/js/`, `tools/` is `runtime/components/base/tools/`,
`IronCalc/` is the fork's checkout beside mr-moqui. Sizes: S under a day, M one to three days, L more.

## 1. What changed from the archived plan

Read against the code on Nov 27, 2026:

- **No Tabulator ESM build is needed.** The vendored `tabulator.min.js` (6.7.0, copied by
  `webroot/build/libs/refresh.mjs`) is the full UMD build with all modules registered; `Tabulator.moduleBindings`
  reaches them once a table is built. It has a fill handle (`selectableRangeFill`).
- **Conditional formatting results and borders are already drawn**: `get_cells_block` returns each cell's extended
  style. Row heights are not drawn (`fill::layout` sends widths only), and neither are data bars, icons or merges.
- **The edits are mostly there**: `height`, `fill` in every direction, and the engine's border, conditional formatting
  and merge calls (`IronCalc/base/src/user_model/{border,conditional_formatting,merged_cells}.rs`).
- **The evaluation that holds everyone up** is the one inside the edit log's row lock (`calc/together.rs`), not the
  room's own: CG1.3 moves it out before anything moves to a thread.

## 2. Batches

| Batch | What | Size |
|---|---|---|
| CG1 | Small wiring and fixes | S each |
| CG2 | Editing in the browser | S to L |
| CG3 | The terminal | M, one L |
| CG4 | Dashboards | M |
| CG5 | Data, sources and undo | S to L |
| CG6 | Docs and screenshots | S |

The large items are CG2.6 merged cells, CG3.4 the push to the terminal and CG5.7 a search dataset as a data sheet.

### 2.1 CG1: small wiring and fixes

1. **BOSS Insights users read the `calcgrid` dataset.** A new artifact group in mr-boss
   `data/BossSetupData09DatasetAccess.xml` with both `org.moqui.report.dataset.calcgrid` and
   `org.moqui.calc.CalcServices.get#CalcFrame`: each is an own-grant check (`report/lib.rs` `authorize_own`, the
   dataset's service), which Insights' root screen grant does not pass. ALLOW view to the Insights groups, as the
   other dataset grants are. `calcgrid` cannot be a business dataset (`report/business.rs` refuses a `serviceName`),
   and `get#CalcFrame` checks the profile and the share for each caller, so a plain grant is safe. A feature in
   mr-boss `test/Insights/InsightsCalcGrid.spec.js`: the Sales administrator reads a panel over it, which also shows
   the `BOSS_INS_REPORT_ENGINE` deny does not block it.
2. **Schedule from the document.** A `schedules` link key in `tools/template/ReportMount.js` `toolsLinks` and in
   mr-boss `service/ironstone/insights/Insights.js` `reportLinks`; `canSchedule` from `CalcGrid.js` `access()`
   (`list#Schedules` and `store#Schedule` offered, as `ReportDelivery.js` checks); a Schedule button in
   `CalcGrid.qvue` for a saved document, to `Schedules?definitionId=`, which preselects it. A link and not a dialog in
   CalcGrid, so that Insights' recipient cap (added by its ScheduledReports area) applies.
3. **No evaluation inside the edit log's row lock.** `together::edit` evaluates in `follow()` while `log::locked`
   holds the head row, and again after it; `save` does the same. Every editor on every server waits behind that
   evaluation. Apply the rows with evaluation paused inside the lock, and evaluate once after it.
4. **A slow-calculation warning.** The 250 ms in `Grid.qvue` and `Dashboards/CalcGrid.qvue` becomes the conf setting
   `calc@slow-ms` (MoquiDefaultConf.xml, its XSD attribute and conf schema entry), sent in the room's state. Over it,
   an author sees a warning with the time, and the server logs one line naming the document and the time. The
   terminal's part is in CG3.
5. **The calamine fallback.** Check, and fix, a sheet whose first used column is not A opening shifted left
   (`calc/service.rs` `workbook_model` places values at `column + 1` without the range's start); read the file once,
   not once per sheet. A test with a sheet that starts at C3.
6. **`sales` for the other sales roles.** Read grants of the profile to `BOSS_SALES_MANAGER` and `BOSS_SALES_READ`
   beside `BOSS_SALES_ADMIN` (mr-boss `BossSetupData01GroupsAndLaunchers.xml`). Authoring stays with
   `CALC_GRID_AUTHOR`.

### 2.2 CG2: editing in the browser

In `js/WebrootCalc.js` and `calc/room.rs`, both screens.

1. **The fill handle by dragging (S).** `selectableRangeFill: true`; in `tableBuilt` the table's
   `selectRange.fillHandle.handleMouseUp` is replaced by one that sends the `fill` edit (`toRow` or `toColumn`, from
   `extendAlongAxis`), as `fillDown()` does. It uses Tabulator 6.7.0's internals, so a client spec pins it.
2. **Clicking references into a formula (M).** While the formula bar's text starts with `=` and the caret follows an
   operator, `(`, `,` or `:`, a click or drag on the grid inserts the reference at the caret
   (`calc.rangeName`): a capture-phase `mousedown` keeps the bar's focus, `selectRange.initializeFocus` is overridden
   for each table, the cell being edited is pinned so `readContent` does not overwrite the text, and another sheet's
   tab can be chosen. **A reference into a data sheet is inserted as `Table[column]`** (`calc.tableName`): the log
   replays the text as typed in rooms of other scopes, whose data has other row counts. The formula bar first; the
   in-cell editor after.
3. **Row heights (S to M).** `fill::layout` sends each sheet's custom heights; the view sets them on its rows;
   `resizableRows` with `rowResized` sends the `height` edit, as `columnResized` sends `width`.
4. **Borders (S to M).** A `border` edit over an area through `UserModel::set_area_with_border` (`BorderArea`: all,
   outer, inner, top, bottom, left, right, none; style and colour). Its undo takes the area and a one-cell ring around
   it, since the neighbours' borders change too. A toolbar menu.
5. **Conditional formatting (M).** Edits to add, change, delete and reorder rules (`add_conditional_formatting` and
   the others), each with its undo; a rules dialog for the selection (a cell's value, text that contains, top and
   bottom, a colour scale); data bars and icons drawn from the extended style.
6. **Merged cells (L).** `get#CalcCells` and the layout carry the merges that meet the area. A merge across columns:
   the anchor cell overflows and the covered cells are blank. Down rows: each covered row draws its own slice of the
   box, since Tabulator renders rows as they scroll into view. `merge` and `unmerge` edits; a merge's undo is an
   unmerge and what it emptied; a row deletion's undo puts merges back.

### 2.3 CG3: the terminal

In `tui/components/calc.rs` unless named.

1. **A sheet's colours (M).** `Span` and `Look` take an optional foreground and background colour and italic
   (`tui/components/mod.rs`, `tui/ui/paint.rs`), brought to the terminal's colours by `theme::sgr_color`, with a
   contrast rule against the theme's background. `cells()` keeps the font colour and the fill.
2. **Merged cells (M, after CG2.6).** The anchor's text spans the merged columns, the covered cells are blank, and the
   cursor moves to the anchor.
3. **Editing a chart (M).** A chart target in the editing menu: add from the selection, change its type, title and
   series, remove; sent as the `charts` edit, as the browser does.
4. **A push to the terminal (L).** A component says which topics it wants (`calc:<room>`) and is given their frames;
   the session routes a push by topic (`tui/session_loop.rs`, `shell.rs` `on_push`). The link sends `join`, `select`
   and `leave`: the remote link through a channel into its push loop (`moqui-host/src/tui_remote.rs`), joining again
   after it reconnects; the in-process link through a sink beside the socket's for a member of the room
   (`moqui-host/src/notws/calc.rs`). The 1.5 s poll stays for links that cannot push.
5. **The calculation time (S, from CG1.4).** The sheet's status line shows the time of the last evaluation, and a
   warning past `calc@slow-ms`.

### 2.4 CG4: dashboards

1. **A dataset's parameters in the panel editor (M).** A dataset's body declares its `parameters`; `calcgrid.json`
   names the document (from `list#Definitions` of type calcgrid), the range and the profile (from `get#CalcProfiles`).
   `list#Datasets` and `get#Dataset` return them; `m-report-definition-editor` (`js/WebrootReport.js`) and the
   terminal's editor show a field for each. Add to dashboard writes `profileId` when one is chosen.
2. **A document's chart as a panel (M).** Each chart in the body gets a stable `id`. `get#CalcFrame` takes `chart` and
   answers the chart as a frame (its labels and a column for each series, from `Room::charts`) with its type and title,
   which the bar, line, area and pie panels draw; a bar chart across (`indexAxis: "y"`) is an option there.
3. **A workbook uploaded under a profile (M).** A `calcUpload` transition in `Dashboards/CalcGrid.xml` and
   `workbookBytes` on `open#CalcDocument`, which takes a workbook together with a `source`: the workbook is the
   overlay and the offered entity the data sheet (`fill::fill` adds a sheet the workbook lacks). An author only; the
   32 MB file and 64 MiB inflated limits; a sheet or table named as a data sheet is refused; the 8 MB overlay limit
   on save. A file field in the New dialog. Changes `docs/security/Attack-Surface-Tools.md` (a workbook is refused
   under a profile today) and the guide.

### 2.5 CG5: data, sources and undo

1. **Data sheets read through a cursor (S to M).** `grid::each_row` in `crates/moqui-tools/src/grid.rs`: an entity
   through the find's iterator, as `grid::export` reads it, and SQL as one capped query. `read_sheet` uses it and
   stops at 100,001 rows. Open and refresh at 100,000 rows measured before and after.
2. **Undo after a refresh moved the data (M).** When a refresh changes a data sheet's row count, `Room::refresh`
   rewrites the formulas it took for undo through the engine's displacement (`to_string_displaced` with
   `DisplaceData::Row`), so an undo restores ranges over the data as it is now.
3. **Formulas from xls, xlsb and ods (M).** A pass of their own through calamine's `worksheet_formula`, written into
   the model as `=` and the text; ods's OpenFormula (`of:=SUM([.A1:.A3])`) translated; an xls formula taken only where
   calamine decodes it, else its value. A test file for each format. No styles.
4. **More access profiles (S each).** `accounting` (Invoice, Payment, FinancialAccount, AcctgTrans, TimePeriod, for
   the `BOSS_ACCOUNTING_*` groups) and `marketing` (campaigns, contact lists, segments), in mr-mantle `data/10-seed`
   as `Sales.xml` defines `sales`, granted in mr-boss; every offered entity has an organization filter
   (`OrganizationFilters.xml`). A spec each, as `CalcGridSales.spec.js`. Not now: purchasing (`sales`'s `OrderPart`
   filter already reads the organization's purchase orders), warehouse (`Asset` has no organization filter), projects
   (no filters yet), people (a privacy review first).
5. **A search dataset query as a data sheet (L).** A `{type: "dataset", datasetId, query}` source, read by calling
   `run#Query` by name as the reader, under the dataset's own policy; a service dataset is refused. A profile offers
   `org.moqui.calc.dataset.<id>`. The scope fingerprint (`calc/scope.rs`) takes the reader's effective dataset row
   filter, from a new report service that answers it, or two organizations' readers would share a room.

### 2.6 CG6: docs and screenshots

The guides change with each batch. Screenshots for the guide are taken by hand (a `page.screenshot` in a run on demo
data, never a kept spec) and saved to `docs/images/`: Calc mode, CalcGrid with two people editing, a chart panel.

## 3. Tests

| Batch | Tests |
|---|---|
| CG1 | `InsightsCalcGrid.spec.js` (the dataset), `tools/test/CalcGrid.spec.js` (schedule link, warning state), `cargo test -p moqui-calc` (the lock, the C3 sheet) |
| CG2 | `cargo test -p moqui-calc` (border, rules and merge edits and their undo), `WebrootCalc.spec.js` (fill handle, pointing), the CalcGrid acceptance walk where it changes |
| CG3 | `cargo test -p moqui-tui` (`calc_tests.rs`), `moqui-host` `notws::calc` and `monitor_tui` |
| CG4 | `cargo test -p moqui-report`, `tools/test/CalcGrid.spec.js` (chart frame, upload under a profile) |
| CG5 | `cargo test -p moqui-calc` and `-p moqui-tools`, `CalcGridSales.spec.js` and a spec for each new profile |

Focused tests before each batch's commits; `./moq test-framework` and `./moq test-apps` once when the batches are in.

## 4. Documents to change

`docs/tools/Database.md` (Calc mode, CalcGrid and access profiles), `docs/ui/Qvt-Client.md#calcgrid-rooms`,
`docs/ui/TUI.md#components`, `docs/tools/Monitoring.md` (the dataset's parameters, chart panels),
`docs/security/Attack-Surface-Tools.md` (the upload), and this plan's status as each batch lands.
