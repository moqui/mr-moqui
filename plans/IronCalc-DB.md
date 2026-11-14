# IronCalc DB: database-driven spreadsheets

Active plan since 2026-11-13 (a proposal from 2026-11-09); Phases 1 to 3 built, with editing together, the scheduled export of a whole document and the terminal sheet view ([Phase 3b](#phase-3b-editing-together-the-scheduled-export-and-the-terminal)). It is shown in two places ([Where it is shown](#where-it-is-shown)): as a Calc mode of the existing Tools › Data › Grid screen, and as CalcGrid in the Dashboards app, limited by access profiles whose authorization records carry entity filters ([Access and row scope](#access-and-row-scope)). Facts about other projects come from reading their source and docs on 2026-11-09; nothing here was built or run, and the items marked "to verify" need the spike of Phase 0 before any later phase is committed to. The engine runs in the server and the browser is a thin client ([Thin client](#thin-client)). Phases 0 to 3 are for v0.1.0. What was settled is in [Decisions](#decisions); Phases 1 to 3 are built (the engine is the xlsx writer, `calculate#Spreadsheet`, Calc mode in Tools › Data › Grid and CalcGrid in Dashboards under access profiles, edited together by the people who have a document open, a range of a document as a report dataset for dashboard panels and scheduled reports, a whole document as a scheduled Excel export, and a sheet in the terminal). Phase 0, the spike, was run 2026-11-13 ([Phase 0 results](#phase-0-results)): it works, full recalculation sets the limit (a hundred aggregate formulas over 100,000 rows take seconds on every edit), and several points of the design changed with what the engine does.

## The idea

A spreadsheet for business users whose data comes from the database, not from a file:

- **A data sheet** filled from the relational database through the Entity Facade: an entity, a view-entity or a `DbViewEntity` view with query conditions (the data grid's sources). The query defines which records are in the sheet; filtering and sorting are changes to the query. The rows are the ones the viewer may read: the entity's authorization and the entity filters of the record that granted it.
- **An overlay document** on top of that data: more sheets with formulas, formatting, conditional formatting, summaries and charts. The overlay is saved; the data is refilled when the document is opened or refreshed.
- **Files in and out**: open an XLSX (and through calamine the older formats), save or export the result as XLSX with its charts and images, and run the same document on a schedule.

It is the analytics view of the data the Tools data grid edits as records, in the same screen, and CalcGrid gives Dashboards and the applications one spreadsheet component for user-authored sheets over the relational database, beside the report tools there that read the search indexes (SearchFacade).

## Where it is shown

One engine, one document type and one client sheet component, in two screens.

### Tools › Data › Grid: Calc mode

The viewer and editor are built into [`Grid.xml`](../runtime/components/base/tools/screen/Tools/Data/Grid.xml) and `Grid.qvue`, not a screen of their own. The grid gets two modes over the same source:

- **Records** (as now): the rows of the source edited in place and saved as records. Nothing changes here, and it stays stateless on the server.
- **Calc**: the source as the read-only data sheet of a workbook, with the overlay sheets beside it. Switching to Calc keeps the source, its conditions, filter, sort and columns; they become the document's query definition, which is the grid's own source JSON, so the engine fills the sheet with the reader the grid uses (`moqui-tools` `grid.rs`) and its refusals (restricted entities, password fields) hold.
- **Every grid source** can be a data sheet: an entity, a view, a data document, a SQL result (`SQL_RUNNER_WEB`, as now) and an uploaded file. An XLSX opens as a workbook, with its formulas and styles, not only as rows.
- **A document** is saved and opened from the grid's view list, beside the saved views (`moqui.database.GridView`). It is a report definition of type `calcgrid`, the same documents CalcGrid lists. A document not saved is a scratch workbook that lives as long as its room.
- **Access** is the Tools app's (`TOOLS_APP`, `ADMIN`): every entity the grid offers, no entity filter, as the grid has now.
- **The terminal render** is the paged list, with a link to Calc: the workbook as the `calcgrid` sheet of `moqui-tui` (Phase 3b).

### Dashboards › CalcGrid

`Dashboards/CalcGrid.xml` (with `CalcGrid.qvue`) is the same sheet component for people who are not administrators: spreadsheets and queries over the relational database, embedded where the Dashboards app's other tools read the search indexes.

- **The list** shows the CalcGrid documents the viewer may read, with new, copy, share, star and delete, as the dashboard list does: a document is a report definition of type `calcgrid`, so its sharing, versions and git sync are the definitions'.
- **View or author.** A profile granted for view opens the documents shared with the user, refreshes and exports them; granted for update it also authors documents. Data sheets are read only for everyone.
- **A document** opens in the sheet view. Its sources are entities and views only, chosen from what the viewer's access profile offers; no SQL source, and a file only as an upload into the overlay.
- **Embedding.** A `calcgrid` panel shows a sheet, a named range or a chart of a document on a dashboard, so it is in playlists, the wall display, report previews and scheduled reports with the other panels. A panel is read only and links to the document.
- **Mounting.** An app shows CalcGrid under its own root as it shows the other Dashboards screens ([Mounting the report screens](../docs/tools/Monitoring.md#mounting-the-report-screens)); the mount's policy can fix the access profile. BOSS shows it under Insights, with the `sales` profile for its sales groups.
- **Not here:** record editing (that is the Tools grid's Records mode and the applications' own screens).

## Access and row scope

The Tools grid is for administrators. CalcGrid opens entity data to other people, so what they can read is set in the authorization data, with entity filters, and not in the document.

### Access profiles

An access profile is an artifact group, its authorization records and their filter sets; no new entity.

| Part | What it is |
|---|---|
| Profile artifact | `org.moqui.calc.profile.<profileId>`, a member of the profile's `ArtifactGroup` with `inheritAuthz` Y. The engine enters it before it reads a source, so every find below is authorized by the profile's `ArtifactAuthz` record and gets that record's filters, whatever other grants the user holds. |
| Offered entities | The group's `AT_ENTITY` members (names or patterns). The engine refuses an entity or view that is not one of them, and a view with a member entity that is not. The source picker lists only these. |
| Row scope | `ArtifactAuthzFilter` rows on the profile's `ArtifactAuthz` records, naming `EntityFilterSet`s. The filters are applied by the Entity Facade on each find ([`filter.rs`](../crates/moqui-core/src/user/filter.rs)), also through a view's aliases; a filter that cannot be applied fails closed. |
| Who holds it | The user groups of the `ArtifactAuthz` records. The same group of entities can be granted to one user group with a filter set and to another without. |

- **CalcGrid's own artifacts** (the screen, its services, the room) are a group of their own, `CALC_GRID`, granted with each profile: holding a profile is what opens CalcGrid. The Dashboards app's grant (`DASHBOARDS_APP`) covers the screen for `ADMIN`.
- **A user with several profiles** chooses one in the screen (the widest first); a mount can fix it. The profile is part of what a room is keyed by.
- **Checked on every call**, in the engine and not only in the screen: a room join, a viewport, an edit, a refresh and an export each run as the calling user under their profile. The Tools grid services stay closed to these users (they check `TOOLS_APP`); the engine calls the grid's reader below that check, with its own.
- **A scoped profile needs a filter for each scoped entity.** `check#CalcProfile` lists the entities a profile offers that no filter of its sets matches, for review in System › Access (Row rules); such an entity is read whole, which is right for a product's name and wrong for an order.

### The first two profiles

| Profile | Where | Offers | Filters | Granted to |
|---|---|---|---|---|
| `admin` | `tools/data/MonitoringSecurityData.xml` | Every entity the grid offers (`*`; the grid's refusals still hold) | None | `ADMIN`, as the Tools app |
| `sales` | mr-mantle (the group and its entities), mr-boss (the grants) | Orders, order parts and items, returns, invoices and their items, payments applied to them, customers (party, organization, person, their roles and contact data), products and categories, stores, and the views over these; view only | `MANTLE_USER_ORG`: rows of the organizations the user is associated with (`filterOrgIds`), and `MANTLE_ACTIVE_ORG` where the user has chosen one | The BOSS sales groups (`BOSS_SALES_ADMIN` and the sales roles that read orders) |

- The `sales` list is exact names where a package holds entities that are not sales data, patterns only where the whole package is.
- The user's organizations are set as the applications set them (Mantle's user organization setup, which fills `filterOrgIds` in the user context); CalcGrid's root calls it when it is present. With no organization the filters match nothing.
- More profiles (purchasing, warehouse, accounting) are data in their applications, following `sales`; none is planned here.

### What a scope changes in the design

- **A room is one scope.** A workbook in memory holds rows already read, so viewers who may see different rows cannot share it. A room is keyed by the document, the profile and the scope values the profile's filters read (for `sales`, the organizations), so people with the same scope share a room and co-edit, and others get their own with the same overlay. Overlay edits reach the other rooms of a document through the diff queue, which Phase 0 showed to work between models with different data.
- **Nothing computed is stored.** The stored document is the overlay without the data sheets, without cached values of formulas that read them and without the data's texts in the workbook's shared strings ([Phase 0 results](#rooms-the-stored-form-and-scopes)). Opening fills the data as the viewer and evaluates before a cell is sent, so one viewer's totals are never shown to another.
- **A scheduled export** runs as the schedule's owner under the owner's profile, and is delivered to others only as report delivery allows ([Report delivery](../docs/tools/Monitoring.md#report-delivery)).
- **A dashboard panel** fills as the dashboard's viewer, as its other panels do.
- **An exported XLSX** holds the rows its exporter could read; it is a file from then on, as an export of the grid is.
- **Sharing a document** (with a group, as a dashboard is shared) shares the overlay and the query, never rows: a reader without a profile that offers the document's entities gets "Not available" in place of the data sheet.

The name works out well: IronCalc beside Moqui and IronStone.

## What was looked at

| Option | Outcome |
|---|---|
| Univer | No. Open core with the suite parts paid (Slides left open source 2026-11-04), React and its own design system, about 11 MB of scripts for a sheet. |
| Jspreadsheet CE | No. The free edition of a paid product, in maintenance; a second widget kit (jSuites); no cross-sheet formulas, no XLSX, a minified formula engine from 2023. |
| fortune-sheet, x-spreadsheet, Luckysheet | Slowing, dead, archived. |
| HyperFormula, Teable, NocoDB, OnlyOffice, Collabora | Licence or shape does not fit (GPL or source-available; whole applications or iframe suites). |
| Casual Office | One person, begun May 2026, Sheets is a Univer fork. Worth watching, not adopting. |
| **IronCalc** | **Chosen.** A spreadsheet engine in Rust, MIT or Apache-2.0, with XLSX read and write and a WASM build. |

## The pieces

### IronCalc: the engine

[IronCalc](https://github.com/ironcalc/IronCalc) at v0.9.1 (October 2026), NLnet funded, three main authors, about 75,000 lines of engine with 64,000 lines of tests.

What it gives us:

- **In our process.** The `ironcalc_base` crate has few dependencies (serde, chrono, bitcode, csv, statrs); the `ironcalc` crate adds XLSX on `zip` and `quick-xml`, which the workspace already builds. The same engine compiles to WASM for the browser, which this proposal does not need at first.
- **A read made for a remote view.** `get_cells_block(sheet, row, column, width, height)` returns a rectangle of cells in one call: the distinct styles once (conditional formats and neighbours' borders already applied), a style index for each cell, the formatted text of the non-empty cells, and fills as runs. It serializes with serde.
- **Storage made for data.** Cells are held by position in row chunks, a row of plain numbers at eight bytes each, with a bulk `set_row`.
- **A spreadsheet's worth of features.** About 500 functions, dynamic arrays, defined names, Excel tables with structured references (`Orders[amount]`), conditional formatting, merged cells, number formats, locales, languages and time zones.
- **An editing layer.** `UserModel` has undo and redo, autofill, clipboard, and a diff queue (`flush_send_queue`, `apply_external_diffs`) that keeps two copies of a workbook in step.
- **A compact stored form.** `to_bytes` and `from_bytes`.

What it does not have:

- **Charts** (planned after 1.0), **pivot tables** (planned), **images**.
- **Collaboration** (their first priority after 1.0; a CRDT layer over the present model is in progress on a branch, [issue 349](https://github.com/ironcalc/IronCalc/issues/349)). They have said a plain Yjs log does not suit them, so expect their own CRDT.
- **Autofilter, sort, data validation, sheet protection, custom functions** or any hook for outside data. None was found in the source.
- **Incremental recalculation.** Every change evaluates every formula in the workbook; `pause_evaluation` batches changes. Fine for data with a modest number of formulas, a cost for a hundred thousand rows of them.
- **A stable API.** It is before 1.0, errors are strings, and `Model` carries a lifetime parameter.

### Tabulator: the grid

The grid stays Tabulator, which is already vendored (6.7.0) and already drives [`Grid.qvue`](../runtime/components/base/tools/screen/Tools/Data/Grid.qvue) with range selection, range copy and paste, undo, frozen columns and remote loading. IronCalc's own web widget is React with a canvas grid and is not used.

Tabulator is the view and IronCalc the model: an edit goes to the engine in the server, and a formatter draws each visible cell from the block of cells the server sent. IronCalc reports no list of changed cells, so the visible rows are sent again and redrawn after an edit.

Tabulator's extension points cover nearly all of it without a change to Tabulator:

| Piece | How |
|---|---|
| Values, styles, borders, conditional formatting | A formatter that reads the block the server sent |
| Edits | A custom editor and `cellEdited`, sent to the server |
| Formula bar, sheet tabs | Our own Quasar components beside the table |
| Picking references by clicking while editing a formula | A custom editor and a module that takes the clicks first (fiddly: the range module wants them too) |
| Undo and redo | IronCalc's, in the server, with Tabulator's key actions rebound through `extendModule` |
| Copy and paste | A custom paste action, as `Grid.qvue` has now |
| Widths, heights, freeze | The existing modules, synced to the engine by events |
| Fill handle | A custom module |
| Merged cells | No clean fit: an overlay above the grid, or a custom renderer |

Two things to know:

- **The build.** The vendored UMD file exposes only the `Tabulator` class. Writing a module or a renderer needs the `Module` and `Renderer` base classes, which are in the ESM build; that is a change to `libs/refresh.mjs`, not to Tabulator.
- **Size.** Tabulator holds an object for each row it has loaded, and loads rows as the user scrolls (`progressiveLoad: 'scroll'`, as `Grid.qvue` does). It draws DOM, slower than a canvas and enough at these sizes. To verify: jumping to a far row without loading the rows between.

Tabulator's own `spreadsheet` module (lettered columns, row numbers, array data, sheet tabs) is in the build and was not exercised. To verify: whether it helps or gets in the way once the engine owns the data.

### TironCalc: the terminal reference

[TironCalc](https://github.com/ironcalc/TironCalc) is one 1,810-line file on ratatui. `moqui-tui` uses Revue, so it is a reference and not code to take: it shows the engine calls for selection, editing, undo, merged cells, freeze panes, styles, sheets and the warnings before saving a workbook another application made.

### The XLSX crates

| | calamine | rust_xlsxwriter | IronCalc |
|---|---|---|---|
| Direction | Read | Write | Read, edit, write |
| Formats | xlsx, xlsm, xlsb, xls, ods | xlsx | xlsx |
| Formulas | Stored values | Writes the text | Parses and calculates |
| Charts, images | No | Yes | No |
| Used for | ETL, the grid's file source, `SpreadsheetServices`' reads | Test workbooks only, since Phase 1 | The xlsx render mode, the grid's export, scheduled reports, `calculate#Spreadsheet` |

- **calamine stays.** IronCalc reads only xlsx, and the import formats must not be lost. calamine can also be the way in for the older formats: load values and formula text into an IronCalc model, without styles.
- **rust_xlsxwriter is gone from the build** (Phase 1). What we wrote was little: worksheets and their names, strings, numbers, number formats, alignment, a header fill, freeze panes and fitted columns, all of which IronCalc's model and exporter cover. It is a dev-dependency for test workbooks.
- **Charts and images** are the real new work in the exporter (Phase 4).

## Architecture

```text
 query definition ──► Entity Facade ──► rows ──┐
 (the grid's source: entity or view,           │ set_row
  conditions; profile and entity filters)      ▼
                                        ┌──────────────┐
 overlay document (stored bytes) ─────► │ IronCalc     │ ──► XLSX, scheduled export
                                        │ model, one   │
 grouped view ─────────► summary sheet ►│ for each room│
                                        │ of a document│
                                        └──────┬───────┘
                          edits, viewport ▲    │ blocks of cells, sheet layout, presence
                          (WebSocket room)│    ▼
                              Tabulator view, formula bar, Chart.js charts
                              moqui-tui view
```

- **The data sheet** is read-only and declared as an Excel table, so overlay formulas name its columns and survive a changing row count. Security is applied where the rows are read ([Access and row scope](#access-and-row-scope)): in Tools the grid's own, in CalcGrid the viewer's profile and its entity filters.
- **Row limit.** Everything is in server memory and recalculated in full, so a data sheet has a cap, as the XLSX download has now (100,000 rows).
- **Summaries.** Grouped totals come from a second data sheet over a view with grouping and aggregate functions (a view-entity, or a `DbViewEntity` built in Tools › Data › Views), under the same filters. That stands in for pivot tables and keeps the heavy work in the database. A report query over a search dataset as a data sheet, under the dataset's own policy, is later (Phase 5).
- **Charts.** A chart is a small definition in the overlay that names ranges. The server sends the values of those ranges; the browser draws them with the Chart.js already vendored; the export writes a native Excel chart.

## Thin client

The engine and the workbook are in the server only. The browser gets the cells on screen and sends edits. From the source this looks like the better first design, ahead of a copy of the engine in the browser:

**What makes it work**

- **`get_cells_block`** (above) was added in October 2026 for IronCalc's own canvas, to replace a call for each cell. It is the read a remote view needs. With it go the row and column sizes, merged cells, frozen counts and links, each one call. A screen of 60 rows by 30 columns is 1,800 style indices, a short style list and the non-empty texts: an estimate of a few tens of kilobytes of JSON before compression.
- **The room is already built.** `moqui-collab` keeps a document for each co-edited record in memory, on the notification WebSocket: `join` (the access check and the load), `apply` (an edit), `tick` (store a while after an edit, drop when the last editor leaves), `save`, and `awareness` frames for presence. A sheet room is the same shape with a workbook in place of a Yjs document, in a crate of its own (`moqui-calc`, on moqui-core through the extension entry as `moqui-collab` is), and one room for each scope of a document ([What a scope changes](#what-a-scope-changes-in-the-design)).
- **Tabulator already loads this way.** `Grid.qvue` asks for rows as the user scrolls. Here a request is a viewport and the answer is a block.
- **A full recalculation costs the same either way.** The engine reports no changed cells, so after an edit the server sends each viewer its viewport again. Sending only what differs from the last block sent is our code in the room, not a change to the engine.

**What it buys**

- **Collaboration without a CRDT.** One model, edits applied in the order the server receives them, every viewer sent the result. Several people can edit the same sheet from the first version, with presence from the awareness frames. IronCalc's CRDT layer is then needed only for offline editing.
- **Rows stay in the server.** A browser holds only the cells it has shown. Export and scheduled runs use the same model.
- **No WASM package** (about 2.3 MB unpacked), no second copy of the workbook to keep in step, and no risk of the browser's engine and the server's differing by a version.
- **One protocol for the web and the terminal.** `moqui-tui` already draws documents a server sends; a sheet view there reads the same blocks.
- **No fork**, of either project.

**What it costs, and what to check**

- **Every committed edit is a round trip.** Typing in the cell is local; the result appears when the server answers. Moving the selection and scrolling over loaded rows are local. Fetch a margin around the viewport so short scrolls do not wait.
- **Formula help.** The function list is static and can be sent once; `formula_completion` is a server call for each keystroke that needs it. To verify: whether that feels quick enough, or the names and signatures should be completed in the browser.
- **One selection and one undo stack in `UserModel`.** Its selection, scroll and window size are for a single user, so the room keeps each user's selection itself and does not use the engine's navigation calls (Tabulator navigates). Undo is the workbook's, shared: one person's undo can take back another's edit. Acceptable at first; an undo stack for each user is engine work.
- **Memory.** A workbook is whole in server memory while open: a row of numbers is eight bytes a cell, so 100,000 rows of 20 numeric columns is about 16 MB, more with text. Drop it when the last viewer leaves and after an idle time, as `tick` does, and rebuild from the overlay and the query.
- **Threads.** `Model<'a>` borrows its locale and language, which are `'static` (held in `OnceLock`s), so a `Model<'static>` can be kept in a room. No `Rc` or `RefCell` was found outside the test clock, so it should be `Send`. To verify by compiling. Evaluation is synchronous and CPU-bound: run it on a blocking thread, one workbook at a time behind its lock.
- **A cluster.** Yjs rooms on two servers merge each other's updates; two IronCalc models cannot. Either one server owns an open workbook and the others forward edits and viewport requests to it, or the owner applies edits and sends its diff queue (`flush_send_queue`, `apply_external_diffs`, made to keep a second model in step) to replicas that answer reads. To verify: which the cluster layer makes simpler. Start with the owner alone.
- **No connection, no sheet.** Where the server is local (desktop, or the browser-local WASM host) that is no loss; a sheet edited offline against a remote server is what the browser engine and the CRDT are for, later.

**The browser engine, later.** If the round trip proves too slow, the WASM engine can be added as a local copy fed by the diff queue, for editing or only for viewing. Nothing in the thin design prevents it: the room's edit messages and the diff queue are the same operations.

- **Reports.** The `calculate` step of the report frames (binary math, running total, moving average, percent of total in `moqui-report/src/frame.rs`) could become a formula for each calculated field. The search aggregations do not change: they are set operations pushed to the database, which a cell engine does not replace.

## Phases

Each phase leaves something usable. Phases 0 to 3 are for v0.1.0 and need no fork of IronCalc and no change to Tabulator; Phases 4 and 5 are after it.

### Phase 0: spike

Done 2026-11-13: [Phase 0 results](#phase-0-results). The list as it was:

- A profile artifact entered by the engine: finds below it carry its `ArtifactAuthz` record and that record's filters, for a user who also holds another grant on the same entity; an entity outside the group is refused; a view with a filtered member is filtered through its alias.
- Overlay edits from one room applied to another room of the same document with different data sheets (`flush_send_queue`, `apply_external_diffs`).
- The stored form without data sheets and without cached values: what `to_bytes` keeps, and what has to be cleared.
- `Grid.qvue` with a second mode: one Tabulator instance configured again, or two, and what the sheet component shares with the record grid.

- Build `ironcalc` into the host behind a feature; load 100,000 rows with `set_row`, add a few hundred formulas, time an evaluation and a save.
- Round-trip a set of real workbooks; list what is lost.
- Structured references to a table whose row count changes on refresh.
- A `Model<'static>` held in a room behind a lock: does it compile as `Send`, and what do an edit, an evaluation and a `get_cells_block` of one screen cost at that size.
- The Calc mode of `Grid.qvue` with Tabulator fed by blocks over the notification WebSocket: a formatter that draws from the block, an editor that sends the edit, the viewport redrawn from the answer. Measure the round trip, and try formula completion through it.
- Loading by viewport in Tabulator, including a jump to a far row.
- The ESM build of Tabulator in `webroot/libs` and a first custom module.

### Phase 1: the engine in the server

Built 2026-11-13.

- **The dependency.** `ironcalc` at the pinned revision `87b684bb` of main (workspace `Cargo.toml`; `deny.toml` allows the repository as a git source), 12 new packages, licenses and sources checked. It is core's `xlsx` feature, not a feature beside it: the engine is now the xlsx writer, so a build with xlsx has it and a build without has neither.
- **`moqui_core::workbook`** (`crates/moqui-core/src/workbook.rs`): `WorkbookWriter` writes typed cells with styles below the engine's formula parser (a text that starts with `=` stays a text), freezes rows, fits columns by character count (the engine measures no font) and gives the engine's `Model` or the `.xlsx`; `open_xlsx` and `model_xlsx` are the engine's reader and writer; the engine is re-exported as `moqui_core::workbook::ironcalc` for the crates on core.
- **The writers are on it:** the `xlsx` render mode and the data grid's export (`XlsxBook`, `screen/xlsx.rs`) and a scheduled report's xlsx (`moqui-report` `schedule.rs`). Their tests read the files back with calamine as before, and a rendered file opens in LibreOffice.
- **`rust_xlsxwriter` is out of the build.** Nothing used sheet protection, and `SpreadsheetServices` only read, so nothing was left on it; it stays as a dev-dependency, for test workbooks from a writer other than the engine.
- **`SpreadsheetServices.calculate#Spreadsheet`** (`service/spreadsheet_calc.rs`): open an xlsx with formulas, set cells (`cellValues`: `B2`, `Inputs!B2`; a text as typed, a number or boolean as it is, null clears), evaluate, and return `readCells` (cells and ranges) as values and as formatted texts, with the calculated workbook as bytes or at a location.
- calamine unchanged.
- Not done here: a spec that calls `calculate#Spreadsheet` through the service facade (its body is tested in the crate, and the registry test covers its location), and `./moq test-framework`.

### Phase 2: the virtual sheet in the Tools grid, read and export

Built 2026-11-13. Guide: [Calc mode](../docs/tools/Database.md#calc-mode).

- **`moqui-calc`** (`crates/moqui-calc`, on moqui-core and moqui-tools, linked under the host's `xlsx` feature):
  - `fill.rs`, the engine work with no ExecutionContext: a data sheet filled with typed rows and declared as a table, rows inserted between the stored form's two empty data rows so ranges stretch, the stored form (rows deleted, the sheet emptied, the shared strings rebuilt, one evaluation), and the layout of the sheets.
  - `room.rs`: the open workbooks, a viewport as the engine's block in JSON, the charts' ranges read, refresh, and the limits (eight rooms a user, twenty minutes idle, 20,000 cells a request).
  - `service.rs`, `org.moqui.calc.CalcServices`: `open#CalcDocument`, `get#CalcCells`, `refresh#CalcDocument`, `save#CalcDocument`, `export#CalcDocument`, `close#CalcDocument`. The data sheets are read through the grid's reader (`grid::columns`, `grid::rows` a page of 1,000 at a time) as the caller, up to 100,000 rows.
- **The document** is a report definition of type `calcgrid`: `dataSheets` (each a name and the grid's spec), `charts`, and `overlay`, the stored form in base64 **in the body**, not in a row beside it as decided: the definition's versions, copy, export and git sync then carry the workbook with no code of their own. At most 8 MB.
- **A workbook file** opens with the engine's reader, and through calamine as values when the engine refuses it.
- **Calc mode** in `Grid.xml` (the `calc*` transitions) and `Grid.qvue`, drawn by `moqui.calc.sheet` (`webroot/js/WebrootCalc.js`): its own Tabulator with placeholder rows and a block cache, sheet tabs, the charts with `m-chart`, Refresh, XLSX, Save document and Documents.
- **Tests:** `cargo test -p moqui-calc` (the fill, the stored form with none of the data's texts, the rooms, the fallback reader), `tools/test/CalcGrid.spec.js` (the transitions: open, cells, export, save, reopen, delete, a file as a workbook), `webroot/test/client/WebrootCalc.spec.js` (the view's helpers) in the regression suite; `tools/test/acceptance/client/CalcGrid.spec.js`, the browser walk, in the acceptance suite.

What differs from the plan, and what is left for later phases:

- **The transport is HTTP transitions, not the notification WebSocket.** Reading needs no push, a request and its answer measured a few milliseconds in Phase 0, and the host is untouched. The WebSocket comes with editing (Phase 3), where other people's edits and presence are pushed.
- **A room is one user's**, keyed by the user and a room id, not shared by the people of a scope: nothing is edited yet, so nothing is lost by it but memory when two people open the same document. Rooms by scope come with the profiles (2b) and editing (3).
- **A room is on the server that opened it.** In a cluster a request that reaches another server gets `ROOM_GONE` and the view opens the document again there; the owner of a workbook is Phase 3's.
- **Charts are shown, not authored:** a chart is in the document's body (`title`, `type`, `categories`, `series` of `name` and `values`, each a range) and is drawn from the values the server reads. The editor for them is Phase 3's.
- **A data sheet is read a page at a time** through the grid's reader, not through a cursor: a large entity costs a query for each 1,000 rows. To measure on real data, and to read through the export's cursor if it is slow.
- **Evaluation is not yet on a thread of its own:** it runs in the request that opens or refreshes, with the room locked.

### Phase 2b: CalcGrid in Dashboards

Built 2026-11-13; what is left is listed at its end. Guide: [CalcGrid and access profiles](../docs/tools/Database.md#calcgrid-and-access-profiles).

- **Access profiles in the engine** (`moqui-calc` `profile.rs`): each `CalcServices` call takes `profileId`. Without it the call is the Tools app's (Phase 2). With it the caller must hold the profile; the source is an entity or view the profile offers (a view with every member offered), read with the profile artifact entered, so the finds get the entity filters of the profile's grant; under a grant with a filter set an offered entity that no filter applies to is refused; authorization is required whatever the caller disabled; and a room is read only under the profile it was opened under. `get#CalcProfiles` and `find#CalcEntities` are new.
- **The data:** `CALC_GRID` (the screen and the services) and `CALC_GRID_AUTHOR` (`org.moqui.calc.document`) in tools, `ADMIN`'s; `CALC_PROFILE_SALES` in mr-mantle (`data/CalcGridProfileData.xml`), with no grant; in mr-boss the grant of the profile to `BOSS_SALES_ADMIN` with `MANTLE_USER_ORG`, `MANTLE_ACTIVE_ORG` and `MANTLE_PRIVATE_CONTACT`, and of `CALC_GRID_AUTHOR`.
- **`Dashboards/CalcGrid.xml`**, `CalcGrid.qvue` and `template/CalcGrid.js`: the documents the viewer reads, one opened as a sheet, the profile chosen (or the mount's, `reportPolicy.calcProfile`), a new document over an entity the profile offers, save with a group of the author's own, delete, refresh, XLSX, and a terminal render of the list. BOSS shows it under Insights by location.
- **Tests:** `mr-mantle/test/order/CalcGridSales.spec.js`, `tools/test/CalcGrid.spec.js` (a feature for the Dashboards screen), `mr-boss/test/Insights/InsightsCalcGrid.spec.js` in the regression suites; a second walk in `tools/test/acceptance/client/CalcGrid.spec.js`.

What differs from the plan:

- **`admin` is not a profile.** It is the Tools app's own access, offered in the Dashboards app to a holder of `TOOLS_APP`. A profile's grant is an always-allow, which passes every deny below it: one over every entity would have opened to `ADMIN` what denies keep to `ADMIN_ADV`.
- **Offered entities are named artifacts**, `org.moqui.calc.entity.<entityName>`, not `AT_ENTITY` members: an entity member is a real grant, and a view needs its plain members offered (an order's header), which would then be readable, unfiltered, wherever an entity is checked on its own.
- **An entity nobody scoped is refused**, not read whole and reported: the engine asks whether a filter of the grant applies to it. So there is no `check#CalcProfile`; the refusal is logged with the profile and the entity.
- **Authoring is its own group** (`CALC_GRID_AUTHOR`), not the action of the grant: the framework checks a service call for all actions, so a view grant of the services opens none of them.
- **A room is still one user's**, so every reader of a shared document has their own, filled as themselves: the scope cannot leak between readers. Rooms shared by scope come with editing.
- **In BOSS a Sales administrator reaches CalcGrid as an Insights user**: no grant names the framework's screen, and Insights' root grant opens the route.

Added after the profile question was settled (2026-11-13: what an application grants a person is what they see in the Dashboards app too, the first profile they hold by default, with a way to switch):

- **The current access.** The profile a person chooses in CalcGrid is kept as theirs (the user preference `calc.profile`, `set#CalcProfile`). A call that names no profile and is not a screen's reads under it while they hold it, else the Tools app's own access when they have it, else the first profile they hold.
- **The dashboard panel is a dataset, not a panel type.** The shipped dataset `calcgrid` (`tools/report/calcgrid.json`) has the service `get#CalcFrame` as its source: a range of a saved document as a table (the first row the columns' names), read as the viewer under the profile the query's parameters name or their current access. The report engine's table, chart and stat panels draw it, and with them playlists, the wall display, report previews and scheduled reports; a scheduled report runs as its owner, so under its owner's access. The viewer's room for the document is read again, its data anew when over a minute old.
- **Tests:** a feature each in `tools/test/CalcGrid.spec.js` (the dataset through `run#Query`, the choice kept) and `mr-mantle/test/order/CalcGridSales.spec.js` (each reader's own rows with no profile named, a profile not held).

Left of 2b:

- **A sheet or a chart of a document as a panel** (the grid with its styles, a chart as its author drew it): the dataset gives a range's values to the report engine's own panels.
- **The panel editor's fields for the dataset's parameters** (Add to dashboard, built with Phase 3, writes the panel).
- ~~**A scheduled export of the whole workbook** as XLSX~~: built in Phase 3b.
- **In BOSS, the dataset for Insights users:** `org.moqui.report.dataset.calcgrid` is not granted to a BOSS group, and Insights reads admitted business datasets under its mount.
- **A file as an upload into the overlay** under a profile, and the guide's screenshots.

### Phase 3: editing

Built 2026-11-13, for one person at a time; editing together followed the same day ([Phase 3b](#phase-3b-editing-together-the-scheduled-export-and-the-terminal)). Guide: [Calc mode](../docs/tools/Database.md#calc-mode).

- **The engine:** `Room::edit` applies a list of edits in order with evaluation paused, then evaluates once: `input`, `paste`, `clear`, `style` (the engine's style paths), `fill` (its autofill), `width`, `height`, `freeze`, rows and columns inserted and deleted, sheets added, renamed and deleted, `charts` (the document's charts, replaced), `undo` and `redo` (the engine's own history). A data sheet's cells, rows, columns and name are refused; its widths, heights and frozen panes are not. `edit#CalcDocument` answers the room's state whether or not an edit was refused; `get#CalcCellContent` answers a cell as typed. Under a profile only an author edits.
- **The view** (`WebrootCalc.js`, both screens): a toolbar (undo, redo, bold, italic, underline, text and fill colour, alignment, number formats, rows and columns, freeze, fill down, Chart), a formula bar that completes function names and the data sheets' table names, a cell's editor on a double click, Delete, Ctrl+Z and Ctrl+Y, a paste of rows of cells, a column's width by its resize, sheets added, renamed and deleted, and charts added from the selection and removed. After an edit every sheet's cells are asked again, and the table is made again only when its shape changed.
- **Add to dashboard** (the 2b gap): in Dashboards › CalcGrid the cells selected of a saved document become a panel (table, bars, pie or number) over the dataset `calcgrid` on one of the viewer's own dashboards.
- **Tests:** `cargo test -p moqui-calc` (the edits, the data sheet's protection, undo, the stored form and a refresh after edits), `tools/test/CalcGrid.spec.js` (the edit transitions; a panel added and read), `webroot/test/client/WebrootCalc.spec.js` (completion, range names, pasted rows) in the regression suite; a third walk in `tools/test/acceptance/client/CalcGrid.spec.js` (a sheet added, a formula typed in the bar, bold, Undo).

Not built, of the plan's Phase 3:

- ~~**Editing together**~~: built in Phase 3b.
- **Picking references by clicking** while a formula is typed, and the **fill handle** by dragging (Fill down is in the menu): each needs a Tabulator module, so the ESM build.
- **The function list** is a fixed list of about a hundred names in the view: the engine's `functions` module is private, so its 500 names and their arguments cannot be asked for. An upstream request.
- **Conditional formatting, borders and row heights** have no control (they are drawn when a workbook has them); **merged cells** are not drawn as merged.
- **Evaluation on a thread of its own**, and the budget's warning beyond the time shown.
- ~~**A terminal view**~~: built in Phase 3b.

### Phase 3b: editing together, the scheduled export and the terminal

Built 2026-11-13, the three sets the user asked for after Phase 3, from the plan written that day. The user's answers:
unsaved edits are kept until a save (not saved as versions on their own); readers see the authors' edits as they are
made (one room per scope, not a room of saved versions for readers); and the terminal reads the others' edits by
polling, not by a push to its components. Guides: [Calc mode](../docs/tools/Database.md#calc-mode),
[CalcGrid rooms](../docs/ui/Qvt-Client.md#calcgrid-rooms), [Report delivery](../docs/tools/Monitoring.md#report-delivery),
[TUI components](../docs/ui/TUI.md#components).

**The scope** (`moqui-calc` `scope.rs`, `get#CalcScope`): a fingerprint of everything that decides a reader's rows, the
access, the time zone, and for each data sheet whether it is readable, its source, the columns shown and the condition
the entity filters add, with its values (in an order that does not depend on how it was made). It decides who shares a
room and who may be sent a scheduled export. CalcGrid now calls the application's user organization setup itself when
it enters a profile with no screen before it (a scheduled run, a report's scope check): without it every `sales` reader
outside a screen read no organization.

**Editing together** (`room.rs`, `log.rs`, `together.rs`; `moqui-host` `notws/calc.rs`; `WebrootCalc.js`):

- **A room for each scope.** A saved document's room id is made from the document, the access and the scope, so the
  same scope on any server names the same room; a room has members, and a call finds it only for one. A scratch
  workbook is its opener's, and saved it becomes the document's room for their scope (`roomId` changes).
- **The edit log**, `moqui.calc.CalcDocumentHead` and `CalcDocumentEdit` (`CalcEntities.xml`; an EECA drops them with
  the definition). An edit locks the head row, brings its room up to the last row, applies itself and is written as the
  next row; every room of the document, of every scope and server, applies the rows in that order over its own data.
  A room is the saved version and the rows after it. A version saved outside CalcGrid begins the log again.
- **Not the engine's diff queue.** Phase 0 relayed edits between rooms with `flush_send_queue`, but the engine's undo
  diffs carry the cells they replaced: a formula's value computed from one scope's rows and an index into that room's
  texts, which holds the data sheet's. Applied to another scope's room it restores the wrong text or the other scope's
  value, and written to the log it stores computed data. `Diff` is crate-private, so it cannot be cleaned from outside.
  The log holds the edits as the view sent them (a formula as typed), and undo is ours.
- **Undo for each person** (Phase 5's, brought forward): before an edit each room takes what it will change (contents
  as typed and styles, sizes, a sheet, and for a deletion every formula, which it turns into `#REF!`), and an undo row
  names the edit it takes back, which each room undoes with what it took. Undo goes back to the last save.
- **No owner in a cluster.** The database's row lock orders the edits, so every server holds its own rooms; a server
  that dies loses nothing. `calc-update` tells every server to bring its rooms up and its viewers; a room that cannot
  follow (behind a revert, rows gone) is dropped and its viewers open it again.
- **Push and presence** on the notification socket: `join` (only into a room the user has open), `select`, `leave`;
  `joined`, `peer`, `select`, `changed`. Presence is by document across scopes and servers (`calc-presence`), names and
  places only. The view outlines the others' selections in their colours, names them, reads the room again on another's
  edit (`get#CalcState`), and leaves a cell being typed in alone until it is set.
- **Discard** (`revert#CalcDocument`) takes every unsaved edit back, everyone's. Any author edits a document they read;
  its owner saves it.

**The scheduled export** (`render#CalcDocument`; `moqui-report` `schedule.rs`, `delivery.rs`; `Reports.xml`,
`ReportDelivery.js`): a calcgrid definition is schedulable, in Excel only, rendered with no room from its saved version
as the owner, every data sheet whole or the run is refused, with no picture in the email. Its schedules are business
ones, and its part of the scope fingerprint is `get#CalcScope` as the recipient, so another organization's reader is
`scope-differs`.

**The terminal** (`moqui-tui` `components/calc.rs`): the `calcgrid` component, opened by the screen's `tui` text (the
room in its props), reading the cells that fit, a cell's content and the room's state with GETs, posting edits
(`Act::Call`, new), taking a paste (`Component::paste`, new), sized by the terminal's rows (`ComponentState.rows`,
new), polling `calcCells` `since` the view's edit every 1.5 s. Dashboards › CalcGrid's terminal list links each
document; Tools › Data › Grid's links Calc. The export is `calcExport` with `json=true`. The function names a formula
completes are the server's now (`FUNCTIONS`, sent with the opening answer), and the browser takes them too.

**Tests:** `cargo test -p moqui-calc` (two scopes following one log, every edit taken back, per-person undo, the scope's
condition text), `-p moqui-report` (the format rule), `-p moqui-tui` (`calc_tests.rs`, and a session test of `Call`,
`rows` and paste), `moqui-host` `notws::calc` and `monitor_tui` (the list, a sheet, a formula typed and undone);
`tools/test/CalcGrid.spec.js` (the log, undo, peers, discard; a scheduled export read back),
`mr-mantle/test/order/CalcGridSales.spec.js` (two organizations editing one document, the scope a schedule may go to),
`webroot/test/client/WebrootCalc.spec.js` (frames, outlines); a fourth walk in `tools/test/acceptance/client/CalcGrid.spec.js`
(two pages on one document: each other's names and places, an edit, Discard).

Not built of these sets:

- **A push to the terminal.** It polls; a component does not get the socket's frames.
- **Schedule from the document.** A document is scheduled from Reports and Scheduled reports, as a dashboard is; the
  CalcGrid screen has no menu item for it.
- **Undo across a save, and of an edit whose data range moved.** An undo of an edit typed before a refresh restores a
  formula's ranges over the data as they were then.
- **A sheet's colours in the terminal**, merged cells, and editing a chart there (it shows them).

### Phase 4: charts and images in XLSX

**Built 2026-11-13, in the fork** (see [Batch 2](#batch-2-incremental-evaluation-in-place-reload-images-and-charts)): the model (`base::drawings`: images, and column, bar, line, area, pie, doughnut and scatter charts), a versioned stored form so that it could be added, the xlsx export and import, and CalcGrid's charts in its exports (`moqui_calc::charts`, on a sheet named Charts). As proposed, the rest of this section:

The first place the engine itself needs to grow.

- **Images:** an image list on the worksheet, `xl/media` files, a drawing part for each sheet with cell anchors and its relationships, content types, the `<drawing>` element. A few hundred lines.
- **Charts:** the same drawing parts, a chart model and a chart part for each chart. A business subset (column, bar, line, area, pie, scatter; titles, legend, axes, number formats, series colours) is perhaps 1,500 to 2,500 lines, with rust_xlsxwriter's `chart.rs` as the reference for the XML.
- **Import** of both is about the same work again. Without it a workbook opened with charts loses them on save, as now.
- The model structs are what `to_bytes` stores, so this changes the stored form. Agree the chart and image model with upstream first; fork only if that fails or takes too long.
- The way round a fork: bring rust_xlsxwriter back for exports with charts and walk the IronCalc model into it. It means mapping every style by hand and keeping two writers, so it is the fallback.

### Phase 5: as needed

- The engine in the browser as WASM, fed by the diff queue, if the round trip is too slow; and offline editing by merge, on IronCalc's CRDT layer when it exists.
- ~~An undo stack for each user~~: built in Phase 3b.
- Pivot tables, when upstream has them or the summary sheet proves too little.
- A report query over a search dataset as a data sheet, under the dataset's policy.
- More access profiles, in the applications that own the entities.
- Formulas that read entities directly, which needs a custom function hook in the engine.
- Opening xls, xlsb and ods for editing through calamine.
- Incremental recalculation, if the row and formula limits get in the way.

## Phase 0 results

Run 2026-11-13 against IronCalc's main branch at `87b684bb` (2026-11-12), in release, on the development machine. The engine part is a program outside the workspace (not kept in the repository): 100,000 rows of 20 columns (14 numbers, 6 texts) on a data sheet, formulas on an overlay sheet. The browser part is a page with the vendored Tabulator 6.7.0 in headless Chrome against that program over HTTP on the same machine. The access profile is a test in the workspace. Not exercised: the notification WebSocket as the transport, `Grid.qvue` itself, and the cluster.

**Outcome: it works, with one real limit (full recalculation) and several changes to the design below.**

### The engine

| Measured | Result |
|---|---|
| Fill 100,000 rows with `set_row` | 65 ms |
| The model in memory, as `to_bytes` | 15.8 MB, written in 20 ms |
| XLSX of it | 9.9 MB in 0.92 s |
| `get_cells_block`, 60 rows by 30 columns | 0.7 to 0.8 ms on the data sheet (1,200 texts, 70 to 110 KB of JSON before compression), 0.4 ms on the overlay |
| `formula_completion` | 0.012 ms |
| `UserModel<'static>` and `Model<'static>` | `Send`; compiled |
| Evaluation, 300 aggregate formulas over the 100,000 rows | 6.6 s, and the same again for every edit and undo |
| Evaluation, 20 `SUMIFS` over the 100,000 rows | 0.42 s |
| Evaluation at 1,000 rows, the same 300 formulas | 21 ms |

One formula over a 100,000-row column costs, each evaluation: `SUM` 8 ms (83 ns a cell), `SUMIF` and `SUMIFS` 15 to 16 ms, `COUNTIF` on text 32 ms, `SUMPRODUCT` of two columns 42 ms, an exact `VLOOKUP` that reaches the last row 56 ms. A formula that reads a few cells costs nothing measurable, and the data alone costs nothing.

- **Full recalculation is the limit, as expected, and it is on every edit.** An edit to a cell nothing depends on still evaluates everything. A sheet of a few thousand rows is instant with hundreds of formulas; at 100,000 rows about ten aggregate formulas fit in a tenth of a second and a hundred do not fit in a second.
- **What follows for the design:**
  - Summaries stay in the database, as planned: a grouped view as a second data sheet, and overlay formulas over its few rows.
  - The data sheet cap is by evaluation time as well as by rows: the room measures each evaluation, shows it, and warns the author past a budget (about 250 ms).
  - Edits typed in a burst are applied with `pause_evaluation` and evaluated once (50 edits cost one evaluation, measured).
  - Evaluation runs on a blocking thread and viewports are answered from the last evaluated state, so reading is not held up by it.
- **Upstream.** 83 ns a cell for `SUM` over plain numbers is slow for a column of `f64`; a faster path for aggregates over numeric ranges and a dependency graph (so an edit evaluates only what reads it) are the two requests that matter most to us, ahead of those in [Working with upstream](#working-with-upstream).

### What the engine does differently from the proposal

- **`get_cells_block` is not in a release.** It is on main after 0.9.1. Until a release has it, the dependency is a pinned git revision, which the license check has to allow.
- **Bulk loading is below `UserModel`.** `set_row` is on `SheetData`, reached through `Model`'s public `workbook`; `UserModel` gives only a shared reference to its model. So data is filled on a `Model` before it is wrapped: open and refresh build a new `UserModel` from the overlay and the rows, and a refresh ends the undo history. Acceptable; a mutable accessor is a small upstream request.
- **Structured references are fixed ranges after parsing.** `Orders[amount]` is resolved when the formula is typed and stored as `Data!$J$2:$J$100001`; a table whose range changes later does not move it, and there is no call to add or change a table (only the `workbook.tables` map, read when the model is built). What works instead, measured: the stored form keeps two empty data rows, and open inserts rows between them with the engine's `insert_rows` (and save deletes them), which stretches and shrinks every range over the data as Excel does. `ROWS(Orders[amount])` and the sums were right at 100,000 rows and after a refresh to 50,007. The table is still declared, so the author can type `Orders[amount]` and completion offers it. With fewer than two rows the two empty rows remain, so `ROWS` says 2.
- **XLSX export drops tables and cell comments** (2 of 268 of upstream's test workbooks differ after a round trip, by those). The exported data sheet is plain cells, and its formulas are plain ranges, which is what Excel needs.
- **A minimal XLSX does not open.** The one workbook in our own tests (`mr-boss/test/files/Move costs.xlsx`, no `xl/styles.xml`) is refused. Opening through calamine (values and formula text into a new model) is needed in Phase 2 as the fallback, not in Phase 5.
- **No real-world workbooks were on this machine** beyond upstream's 268 test files, so "what is lost" for charts, images, pivot tables, data validation and sheet protection is from the source (none is read or written), not from a run.

### Rooms, the stored form and scopes

- **Overlay edits relay between rooms with different data.** Two models of one document, 1,000 and 400 rows of different values: a formula, a text and a row height set in one, sent as its diff queue (51 bytes for the three) and applied to the other, give the same overlay there, evaluated over the other's data, with its data untouched. Applying does not queue the edits again, and an undo relays the same way. So the open question is answered: rooms of every scope of a document edit together.
- **The stored form** is the room's workbook with the inserted rows deleted, the data sheet emptied and one evaluation: 14 KB for 300 formulas, in 2.5 ms, with every formula's cached value computed from no data.
- **The shared strings must be rebuilt on save.** The workbook keeps one list of every text, the data sheet's included, and emptying the sheet leaves them (2,033 here): customers' names would be in the stored document. Save writes a list of the overlay's own strings only. This is a spec of its own.

### Access profiles

Confirmed in `crates/moqui-core/src/user/filter_tests.rs` (`a_profile_frame_gives_its_filters_to_finds_below_whatever_else_the_user_holds`): a user who holds an unfiltered grant on an entity and its view reads every row; under a pushed profile artifact (`push_checked`, an inheritable always-allow for view with a filter set) the same finds are filtered, on the entity and through the view's alias; off the frame the open grant answers again; a user with no organization gets a term that matches nothing. As the design says, inheritance allows every entity below the frame, so the list of offered entities is the engine's own check against the group's members.

### The browser

| Measured | Result |
|---|---|
| Tabulator built with 100,000 placeholder rows (`{id}` only) | 0.33 s, 28 MB of heap |
| First screen drawn from a block | 32 ms |
| Jump to row 90,000 with nothing loaded between | 49 ms to scroll, 47 to 52 ms for its block |
| Steady scroll, about 80 rows a second | No frame with a row not yet loaded, with a 40-row margin and one request in flight |
| A request and its answer, transport alone | 3.6 ms |
| An edit (20 `SUMIFS` over 100,000 rows) | 422 ms, 421 of them the evaluation; the block after it 42 ms |
| Completion for each keystroke | 0.3 to 0.6 ms a call |

- **Loading by viewport works, and not with `progressiveLoad`.** The table gets a placeholder row for each sheet row and no data; a formatter draws each cell from a cache of blocks, and a scroll asks for the rows in view and a margin, then reformats the rows in view. A far jump needs nothing between. Range selection, the clipboard options and a custom editor work on such a table as they do in `Grid.qvue`.
- **So Calc is its own Tabulator instance and its own component.** It shares the library and the range and clipboard options with the record grid, and nothing of its loading (`ajaxURL`, `progressiveLoad`, remote sort and filter). `Grid.qvue` shows one or the other by mode.
- **The round trip is the evaluation.** Transport and block are a few milliseconds; completion can be a call for each keystroke. The browser engine (Phase 5) would not help an edit: the evaluation would cost the same there.
- **The ESM build is needed for a custom module.** The vendored UMD file exposes `registerModule` and `extendModule` but not the `Module` base class or the module bindings, so a module cannot be written against it. The formatter, the editor and the loader above needed no module; the fill handle and reference picking do.

## Engine speed in the fork

Begun 2026-11-13 in the fork, [moqui/IronCalc](https://github.com/moqui/IronCalc), cloned at `../IronCalc`. The work is commits on the fork's `main`, on top of upstream's main as merged into it, one change for each commit so that each can be offered back on its own; the larger pull requests wait until the work has settled. The workspace builds the fork at a pinned revision (`Cargo.toml`, `deny.toml`).

### The benchmark

`base/examples/data_sheet.rs` in the fork (upstream's `xlsx/examples/deep_chain.rs` and `forward_chain.rs` are the precedent for a timing example):

```
cargo run --release -p ironcalc_base --example data_sheet -- [rows] [mix] [only] [times]
```

- A `Data` sheet of 100,000 rows by 20 columns (an id, six texts, thirteen numbers), a `Numbers` sheet with the thirteen number columns alone, and formulas on the first sheet. The sheets are filled one after the other with rows made at their size, as a table read from a database is: rows built while another sheet was filled row by row lay out of order in memory, and reading a column then cost a third more.
- The memory of the workbook, counted by the allocator and as `Model::memory_usage` gives it; then each of 26 formulas evaluated alone, best of five, with the allocations and the peak memory of one evaluation; then a mix of 300 (thirty each of `SUM`, `AVERAGE`, `MAX`, `COUNT`, `SUMIF`, `SUMIFS`, `COUNTIF`, `COUNTIFS`, `SUMPRODUCT`, `VLOOKUP`).
- With `only`, one formula is evaluated `times` times, for a profiler. With `RELOAD` set, the workbook goes through `to_bytes` and `from_bytes` first, as a stored workbook is opened.

Profiles are stack samples taken with gdb, since `perf` and samply need `kernel.perf_event_paranoid` at 1 and it is at 4. Timings that decide a change are the best of three runs of each build, alternated. The sampler and the runs are in `../quickjs-oxide-bench/work/ironcalc/`.

### Results

| Over 100,000 rows | Upstream `87b684bb` | The fork now |
|---|---|---|
| The workbook in memory | 126.3 MB | **50.1 MB** |
| `SUM` of a column | 13.3 ms | 0.34 ms |
| `SUM` of 13 columns (1.3 million cells) | 26.5 ms | 4.0 ms |
| `SUM` of the 13 columns of the `Numbers` sheet | 25.3 ms | 0.86 ms |
| `AVERAGE`, `MAX`, `COUNT`, `STDEV` of a column | 12 to 13 ms | 0.35 to 0.39 ms |
| `SUMIF` on a text column | 35.0 ms | 1.4 ms |
| `SUMIFS`, two text criteria | 38.5 ms | 2.0 ms |
| `COUNTIF` on a text column | 38.9 ms | 1.1 ms |
| `COUNTIFS`, text and number | 39.3 ms | 1.5 ms |
| `SUMPRODUCT` of two columns | 42.6 ms, 10.7 MB held | 1.4 ms, 1.5 MB held |
| `VLOOKUP`, exact, last row | 54.1 ms | 1.6 ms |
| `SUM(Data!J:J)`, a whole column | 16.7 ms | 0.34 ms |
| **The mix of 300** | **7.52 s**, 67 million allocations | **0.38 s**, 4,928 |

With 0.38 s for 300 aggregates over 100,000 rows, the 250 ms budget for an evaluation (Phase 0 results) holds about 200 such formulas at that size, where before it held ten.

### The commits

Each passes `ironcalc_base`'s tests (2,894 now), the same with `IRONCALC_MAX_DEPTH=1`, the `ironcalc` crate's 246 tests of Excel's workbooks, and upstream's lint. Each new test was also run against the code before its change, to be sure it says what the old code did.

1. **The `data_sheet` example.**
2. **A cell is looked for where its column should be** before the row is searched: in a row filled from column A on, column `c` is at index `c - 1`. The bisection was 80% of `SUM`, `SUMIF` and `SUMPRODUCT`.
3. **Numbers far apart are compared without rounding.** `compare_values` rounded both to 15 digits by formatting them as text and parsing them back, 63% of `VLOOKUP`; only numbers within `scale × 1e-14` of each other need it. (With a test of `MATCH` on close and far numbers.)
4. **A sheet keeps its used area.** `Worksheet::dimension()` walked every cell, on every evaluation, for `COUNTIF` and for any whole column. Now it is found from the ends of each row and kept; a cell put down stretches it, and it is found again only after cells are removed or moved.
5. **The IFS functions judge each text once**, by shared string index, and read constants from the sheet. They copied the text and then made a lower case copy, three allocations for each row.
6. **`SUMPRODUCT` reads a range into its numbers**, not into rows of `ArrayNode` and then rows of `f64`.
7. **Functions over a range of numbers read it a row at a time** (`Model::walk_row`, `for_each_number_in_range`): a row of numbers as a slice, the constants of any other row straight from it with a text lent and not copied, a column of a range as one look. Formulas, spills and errors still go through `evaluate_cell` in the same order, and the walk stops where the function stops, so the same cells are read and put on record. Fourteen functions use it; `COUNT` and the functions on `for_each_value` also stop walking a whole column at the used area, as `SUM` did, where they walked a million rows.
8. **The example fills one sheet after the other** (above).
9. **A row of values with no formula is held as 16-byte constants** (`Row::Constants`), where a cell of any kind takes 48 bytes, for the kinds that hold a formula, and 56 with its column. A data row of 20 is 320 bytes instead of 1,120. `set_row` and the reading of a saved workbook choose it; a formula, or a cell changed in place, makes the row a row of cells as before; it is saved as cells, so the stored form does not change. `SheetData::peek` and `constants_in_row_between` read it without making cells.
10. **`Model::memory_usage`**: the bytes of the cells of each sheet and of the shared strings, from the sizes of the rows (49.8 MB estimated against 50.1 MB counted at 100,000 rows; the parsed formulas, styles and the allocator's overhead are left out). See the touch-points below.

Not run: the bindings' own tests (WASM, Node.js, Python; they build and pass clippy) and `ironcalc_base` on `wasm32-wasip2`, which upstream's CI runs.

### Next: evaluate only what changed

Every edit still evaluates every formula of the workbook. At 0.38 s for the mix, the rows are no longer the limit for a sheet of aggregates; the number of formulas is, and an edit that touches nothing they read costs as much as one that does.

**Upstream, as of 2026-11-13: planned, not started.** [Issue 849](https://github.com/ironcalc/IronCalc/issues/849) (opened April 2026) asks for it. The main author calls it "the biggest problem in IronCalc as of now" (July), said in October that it is "NOT a great issue to pick up" and "definitely not something I want to give to an LLM", needing brainstorming and design first, and on October 29, answering a prototype, that it "might just be the right amount of inspiration I need to finally kick this off". There is no branch for it upstream. In the issue:

- A prototype on [incr](https://github.com/Anyesh/incr), an incremental computation library: one query for each formula, reads recorded in `evaluate_cell`; 0.06 ms against 314 ms for an edit at 1 million rows, about 340 bytes for each formula, and the full pass for spills and cycles. Built with AI agents, as its author says.
- A sketch of the same idea inside the existing evaluator: reads recorded at run time, a map for single cells and a spatial index (R*-tree) for ranges, a dirty set walked from the edit, volatiles always dirty; then an early cutoff by height in the graph.
- [Pull request 1435](https://github.com/ironcalc/IronCalc/pull/1435) (September, from outside the project): `Model::evaluate_incremental(edited_cells)` with an index built by `evaluate()` and a fallback to the full pass near spills and cycles, 0.3 s to 50 µs on 100,000 formulas. Not reviewed, and it no longer merges with main.

What follows for us: the design upstream is converging on is the one below, so the two do not pull apart. The main author wants to write it himself and is wary of contributions written by models, so we do not send this one as a pull request; our use (formulas over data sheets of 100,000 rows, rooms on a server that want the changed cells) is worth adding to the issue, by the user. Our speed commits do not conflict with it, and `walk_row` helps it: the reads of a range now pass through one place, where a read can be recorded as one rectangle. If upstream has not begun in a few weeks, we build it in the fork on the same design, and offer it as a reference rather than a pull request.

The design:

- **Record what each formula read in its last run**, where the reads already pass through one place: `evaluate_cell` for single cells, and the range readers (`walk_row`, `read_row`, `clip_to_used_area`) for rectangles, one record for each range and not for each cell. The evaluation already records reads for the dynamic arrays (`record_seen`, `seen_ranges`), for the root of the recursion; this is the same for every formula, kept across evaluations.
- **An edit marks dirty** the cells it changed, the formulas whose records cover them, and transitively the formulas that read those. An evaluation then runs the dirty formulas only, in the existing pass with the clean ones read as the stored values they are. A formula's references that depend on values (`INDIRECT`, `OFFSET`, `INDEX` returning a reference, a defined name) are covered as they were last resolved: the cell that changes the reference is itself a read, so the formula is dirtied and records its new reads.
- **Always dirty**: the volatile functions (`NOW`, `TODAY`, `RAND`, `RANDBETWEEN`, `RANDARRAY`, `CELL`, `INFO`), which IronCalc has no list of yet, and everything after a structural change (rows or columns inserted or deleted, sheets renamed, names redefined) until a full evaluation has recorded again. Dynamic arrays whose area changes restart as they do now.
- **What changed comes out of it**: the formulas whose values changed are a list of changed cells, which upstream does not have and which the rooms want (below).
- **Its cost**: one record for each formula, mostly one or two ranges for an aggregate; a hundred thousand formulas that each read a few cells is a few MB. An index from a cell to the formulas reading it: single cells in a map, ranges in a list by sheet and column scanned per changed cell, which is cheap for a few hundred ranges.
- **The test**: the existing order-independence oracle in `base/src/test/dynamic_evaluation` evaluates in different orders and compares; the same idea compares an incremental evaluation after each edit of a random sequence with a full one.

### Touch-points for the integration

What `moqui-calc` works around, with what the engine could offer instead. In order of value to us:

| Now | What it costs | Offer |
|---|---|---|
| A refresh builds a new `UserModel` from the overlay and the rows: data can be loaded only on a `Model`, below `UserModel`, which gives only a shared reference to its model | The undo history ends at every refresh, and the whole model is rebuilt from bytes | A bulk load on `UserModel` that replaces the rows of a sheet (`set_row` underneath) and is one step of its history, or a mutable accessor to the model **Built** (batch 2: `replace_rows`, used when the row count is unchanged) |
| Every edit sends the visible rows again, because nothing says which cells changed | A viewport of 60 × 30 for each member of a room, for each edit | The changed cells of the last evaluation (falls out of evaluating only what changed) **Built** (batch 2: `last_changes`, the `changed` areas) |
| ~~No measure of memory~~ | | `Model::memory_usage` (commit 10); the room can show it, and the server can count what its rooms hold |
| Tables cannot be added or resized after load; structured references become fixed ranges. Data sheets keep two stub rows and insert rows between them so that the ranges stretch | `insert_rows` and `delete_rows` over the whole sheet on every open and save | An API to add a table and set its range, moving the formulas that refer to it |
| The shared strings keep every text of every sheet; the stored form empties the data sheets and rebuilds the list itself | A pass over every cell of every sheet on save | Dropping unused shared strings in the engine (`Workbook::compact_shared_strings`) **Built** (batch 2: `compact_shared_strings`) |
| Errors are strings | Messages matched by text | Error types |

### Proposed next, in order

1. Offer the commits upstream, one at a time and the example first, once we have used them for a while. Add our use to issue 849 (the user's call); our incremental evaluation is a working reference for it.
2. ~~The bulk load on `UserModel`~~ and ~~evaluating only what changed~~: built in batch 2.
3. The dependency index smaller for sheets of many one-cell formulas (155 bytes a formula now).
4. The remaining speed in the profiles: `COUNTA` on text still copies each text (100,000 allocations), `MEDIAN` sorts a copy, `VLOOKUP` and `MATCH` hold 4 to 5 MB while they run (an array of the lookup column).

### Batch 2: incremental evaluation, in-place reload, images and charts

Built 2026-11-13 on the fork's `main` (`97efe7e2` to `dbecebb3`), on the user's direction to build these now rather than wait for upstream, following upstream's plan where it has one (issue 849); mr-moqui builds `dbecebb3`.

| Commit | What it does |
|---|---|
| A sheet records what was written to it | `SheetData::take_changes`: the areas written, merged, or that anything may have changed (rows or columns moved, a row hidden) |
| `Model::evaluate_changes` | What each formula read is recorded as its references are resolved (a range is one rectangle; INDIRECT, OFFSET, names and `#` as what they came to; a dynamic array also reads its spill area). An index (single cells in a map, ranges up to 64 columns under each column, wider ones in a list) finds what reads an area. An edit runs what it wrote, what reads that and so on, and the volatile formulas (`Function::is_volatile`), and returns the changed areas. A full evaluation instead before a first one, after cells moved, near arrays, at a cycle, and when more than half of more than 1,024 formulas would run. `UserModel` evaluates this way after every edit (`last_changes`) |
| The early cutoff | The dirty formulas run precedents first; one whose inputs kept their values keeps its own and does not make its readers run (issue 849's second step). The random test, run once with 2,000 sequences of 40 edits, matched a full evaluation at every step |
| `UserModel` history and queue can be off | `set_keeps_history`, `set_keeps_send_queue`: rooms keep their own, and the engine's grew for as long as a room was open |
| `replace_rows` | A stretch of rows loaded again in place, the ranges over it stretching as for rows inserted by hand; `intern_shared_string` |
| Shared strings held once, and dropped | `SharedStringIndex` (a hash, the text read from the list); `compact_shared_strings` |
| A versioned stored form | `base::stored`: a mark and a version; versions 0 and 1 are read through the types they had |
| Images and charts in the model, the xlsx export and the import | See Phase 4; the import says what it left out (`load_from_xlsx_bytes_with_notes`) |

Measured (`data_sheet`, 100,000 rows; the full evaluation of the 300-formula mix is 0.33 s):

| Edit | Time | Formulas run |
|---|---|---|
| A cell nothing reads | 0.001 ms | 0 |
| A cell one formula reads | 0.4 to 0.6 ms | 1 |
| A data cell under every sum | 69 ms | 51 |
| A formula of the overlay | 0.004 ms | 1 |
| The head of a chain of 100,000 formulas | 41 ms (a full evaluation: more than half would run) | all |

The dependency index is 0.05 MB for the mix and 15.5 MB for the chain of 100,000 formulas (155 bytes a formula). 100,000 distinct texts and their cells: 17.6 MB to 13.5 MB.

In mr-moqui: rooms keep no engine history or queue; they evaluate changes only; an edit of values (input, paste, clear, fill) answers the areas it changed, and the document's other rooms tell their views theirs, so a view one edit behind reads only those rows again (`calc.forgetRows` in `WebrootCalc.js`; the terminal keeps its block when none is on it); a refresh with the same number of rows replaces them in place; `memoryBytes` is in every room's state and shown beside the calculation time; exports carry the charts.

Not done in this batch: drawings in `UserModel`'s undo, and charts read from rust_xlsxwriter's files in the tests (the three workbooks of upstream's tests, from Excel and LibreOffice, are read instead).

### Upstream's practices

What a pull request there has to meet, from `CONTRIBUTING.md`, the `Makefile`, the workflows and the history:

- **Talk first.** `CONTRIBUTING.md` asks for an issue or a message before starting, a branch from a clean `main`, and changes that are "properly tested".
- **Lint** (`make lint`): `cargo fmt -- --check`, and `cargo clippy --all-targets --all-features` with `unwrap_used`, `expect_used` and `panic` as warnings and every warning an error. No `unwrap`, `expect` or `panic` outside tests; a test file opens with `#![allow(clippy::unwrap_used)]`. The tree is rustfmt-clean, so `cargo fmt` is run there.
- **Tests in CI**: `cargo test` on Ubuntu and macOS, `ironcalc_base` again on `wasm32-wasip2` under Wasmtime, then the WASM, Node.js and Python bindings and the web app, and a check that `language.bin` is current. Coverage goes to Codecov.
- **Where tests go.** A function or feature has a file in `base/src/test/` (`test_fn_sumifs.rs`), listed in `test/mod.rs`, written with `new_empty_model()`, `model._set("A1", "=...")`, `model.evaluate()` and `model._get_text("A1")`. The storage has its own in `base/src/sheet_data/test.rs`. Results against Excel are workbooks in `xlsx/tests/calc_tests/` (25 folders by function category), one generated test for each file, comparing with the values Excel stored. `IRONCALC_MAX_DEPTH=1 cargo test` runs every test through the evaluation's unwind and replay.
- **No benchmark harness.** There is no `benches/` and no criterion; timing programs are examples, and measurements are quoted in the comments of the code they justify.
- **Commits**: a subject that starts `FIX:` or `UPDATE:`, one sentence, the issue number in parentheses when there is one. Pull requests get a Copilot review that the authors answer.
- **Comments**: doc comments in plain prose that say why, with the measurement where speed is the reason (`sheet_data/mod.rs`, `evaluation.rs`).
- **Direction.** The evaluation and the cell storage were both rewritten for speed in September and October 2026 by the main author, so speed changes fit what upstream is doing. One change for each pull request, each with its numbers from `data_sheet`, and the example itself offered first.

## Decisions

Settled 2026-11-13.

| Decision | Outcome |
|---|---|
| Where the viewer and editor are | In the existing Tools › Data › Grid screen, as a Calc mode beside Records; no separate Tools screen |
| The limited version | CalcGrid in the Dashboards app, the same component, for relational data beside the search-based report tools |
| How it is limited | By authorization data: access profiles (an artifact group, its grants, their entity filter sets), never by the document |
| First profiles | `sales`, the rows of the user's associated organizations; the administrator's is the Tools app's own access, not a profile (Phase 2b) |
| Sources in CalcGrid | Entities and views through the Entity Facade; no SQL; business datasets (search) not a source at first |
| Summaries | A grouped view through the Entity Facade, not search aggregations |
| Where `sales` lives | The artifact group and its entity list in mr-mantle, beside the filter sets; the grants to the BOSS sales groups in mr-boss; CalcGrid shown in BOSS under Insights by its mount |
| Editing in CalcGrid | A holder of a profile opens documents shared with the user, with refresh and export; a holder of `CALC_GRID_AUTHOR` also authors documents (overlay, sources from the profile, charts). Data sheets are read only for everyone. (By the profile's grant action as first decided; a group of its own since Phase 2b, as a service call is checked for all actions) |
| The document's store | A report definition of type `calcgrid` (`moqui.report.Definition`), the overlay bytes in its body (in a row beside it as first decided; changed in Phase 2): sharing with groups, versions, stars, git sync and schedules as dashboards have. The Tools grid lists the same documents |
| Release | Phases 0 to 3 (through editing, in both screens) are for v0.1.0, if the spike holds; charts and images in XLSX (Phase 4) and Phase 5 after |
| Unsaved edits (Phase 3b) | Kept in the document's edit log until a save makes a version; Discard takes them all back. Not saved as versions on their own |
| What readers see (Phase 3b) | The authors' edits as they are made: one room per scope for readers and authors alike |
| The terminal's live edits (Phase 3b) | Read by polling every 1.5 s; no push to its components |
| The order of edits in a cluster (Phase 3b) | The database: a row lock on the document's log head, not an owning server |

## Open questions

None from the plan; the question of rooms of different scopes was answered by Phase 0 (they edit together through the diff queue). Phase 0 left three things not run: the notification WebSocket as the transport, the owner of a workbook in a cluster, and real-world workbooks with charts and pivot tables.

## Working with upstream

There is a fork of IronCalc, [moqui/IronCalc](https://github.com/moqui/IronCalc), for the speed and memory work ([Engine speed in the fork](#engine-speed-in-the-fork)) and the expansions we need; the workspace builds it at a pinned revision of its `main`, and each change is offered back. No fork of Tabulator.

- **IronCalc.** Small requests that make our use easier: sheet protection, a list of changed cells after an evaluation (it would let the room send changes in place of whole viewports), selection and undo kept outside the model for several users, a hook for custom functions, error types. Larger: the chart and image model. They plan charts after 1.0, so our work there is either a contribution or a divergence; talk before writing it.
- **Tabulator.** Extension points only. If merged cells or reference picking need a hook that is missing, propose the hook.
- **Versions.** Build the fork at a pinned revision of its `main`, which takes upstream's main by merge, and move on purpose; the API will change before 1.0.

## Risks

- **A young engine with three authors.** Mitigated by the licence, the size (readable) and the language (ours).
- **Full recalculation.** Sets the row and formula limits; measured in Phase 0.
- **Merged cells in a row-based grid.** May never be good in Tabulator; a data-driven sheet does not need them, an imported workbook may.
- **Round-trip latency** on every edit, the price of the thin client. Measured in Phase 0; the browser engine is the way out.
- **Server memory and CPU** for each open workbook, and one owner in a cluster.
- **Shared undo** among several editors.
- **An open profile.** A profile that offers an entity none of its filters matches shows every row of it. The list of offered entities is explicit, `check#CalcProfile` reports the unmatched ones, and the specs of each profile read as a user of another organization.
- **Two modes in one screen.** `Grid.qvue` is 861 lines for the record grid alone; the sheet component is kept apart from it so the Records mode does not change.
- **Fork drift** if Phase 4 is not taken upstream.
- **Expectations.** It will not be Excel. Say which features are in before users find which are not.
