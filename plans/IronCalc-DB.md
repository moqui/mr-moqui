# IronCalc DB: database-driven spreadsheets

Active plan since 2026-11-13 (a proposal from 2026-11-09); nothing built. It is shown in two places ([Where it is shown](#where-it-is-shown)): as a Calc mode of the existing Tools › Data › Grid screen, and as CalcGrid in the Dashboards app, limited by access profiles whose authorization records carry entity filters ([Access and row scope](#access-and-row-scope)). Facts about other projects come from reading their source and docs on 2026-11-09; nothing here was built or run, and the items marked "to verify" need the spike of Phase 0 before any later phase is committed to. The engine runs in the server and the browser is a thin client ([Thin client](#thin-client)). Phases 0 to 3 are for v0.1.0. What was settled is in [Decisions](#decisions); one question is left to the spike ([Open questions](#open-questions)).

## The idea

A spreadsheet for business users whose data comes from the database, not from a file:

- **A data sheet** filled from the relational database through the Entity Facade: an entity, a view-entity or a `DbViewEntity` view with query conditions (the data grid's sources). The query defines which records are in the sheet; filtering and sorting are changes to the query. The rows are the ones the viewer may read: the entity's authorization and the entity filters of the record that granted it.
- **An overlay document** on top of that data: more sheets with formulas, formatting, conditional formatting, summaries and charts. The overlay is saved; the data is refilled when the document is opened or refreshed.
- **Files in and out**: open an XLSX (and through calamine the older formats), save or export the result as XLSX with its charts and images, and run the same document on a schedule.

It is the analytics view of the data the Tools data grid edits as records, in the same screen, and CalcGrid gives Dashboards and the applications one spreadsheet component for user-authored sheets over the relational database, beside the report tools there that read the search indexes (SearchFacade).

## Where it is shown

One engine, one document type and one client sheet component, in two screens.

### Tools › Data › Grid: Calc mode

The viewer and editor are built into [`Grid.xml`](../runtime/base-component/tools/screen/Tools/Data/Grid.xml) and `Grid.qvue`, not a screen of their own. The grid gets two modes over the same source:

- **Records** (as now): the rows of the source edited in place and saved as records. Nothing changes here, and it stays stateless on the server.
- **Calc**: the source as the read-only data sheet of a workbook, with the overlay sheets beside it. Switching to Calc keeps the source, its conditions, filter, sort and columns; they become the document's query definition, which is the grid's own source JSON, so the engine fills the sheet with the reader the grid uses (`moqui-tools` `grid.rs`) and its refusals (restricted entities, password fields) hold.
- **Every grid source** can be a data sheet: an entity, a view, a data document, a SQL result (`SQL_RUNNER_WEB`, as now) and an uploaded file. An XLSX opens as a workbook, with its formulas and styles, not only as rows.
- **A document** is saved and opened from the grid's view list, beside the saved views (`moqui.database.GridView`). It is a report definition of type `calcgrid`, the same documents CalcGrid lists. A document not saved is a scratch workbook that lives as long as its room.
- **Access** is the Tools app's (`TOOLS_APP`, `ADMIN`): every entity the grid offers, no entity filter, as the grid has now.
- **The terminal render** stays the paged list; a sheet view in `moqui-tui` is Phase 3, if wanted.

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

- **A room is one scope.** A workbook in memory holds rows already read, so viewers who may see different rows cannot share it. A room is keyed by the document, the profile and the scope values the profile's filters read (for `sales`, the organizations), so people with the same scope share a room and co-edit, and others get their own with the same overlay. Overlay edits reach the other rooms of a document through the diff queue ([Thin client](#thin-client)); to verify in Phase 0, and until then a document has one editing room and the others follow it on reload.
- **Nothing computed is stored.** The stored document is the overlay without the data sheets and without cached values of formulas that read them. Opening fills the data as the viewer and evaluates before a cell is sent, so one viewer's totals are never shown to another.
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

The grid stays Tabulator, which is already vendored (6.7.0) and already drives [`Grid.qvue`](../runtime/base-component/tools/screen/Tools/Data/Grid.qvue) with range selection, range copy and paste, undo, frozen columns and remote loading. IronCalc's own web widget is React with a canvas grid and is not used.

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
| Used for | ETL, the grid's file source, `SpreadsheetServices` | The xlsx render mode, `SpreadsheetServices`, scheduled reports | Nothing yet |

- **calamine stays.** IronCalc reads only xlsx, and the import formats must not be lost. calamine can also be the way in for the older formats: load values and formula text into an IronCalc model, without styles.
- **rust_xlsxwriter can go** once IronCalc writes what we write. Today that is little: worksheets and their names, strings, numbers, number formats, freeze panes and sheet protection. IronCalc's model and exporter cover all but protection.
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

Answer the items marked "to verify" and the ones below before committing to the rest.

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

- An `ironcalc` feature beside `xlsx`.
- The xlsx render mode, `SpreadsheetServices` and scheduled reports write through the IronCalc model. Sheet protection is the one missing piece: ask upstream for it, and keep rust_xlsxwriter for the protected exports until it lands.
- A service that opens an XLSX, sets cells, evaluates and returns values or a file: calculation over a template workbook, useful on its own.
- calamine unchanged.

### Phase 2: the virtual sheet in the Tools grid, read and export

- `moqui-calc`: the document (query definitions in the grid's source JSON, the overlay bytes, chart definitions) and its services: create, open, refresh, save, export.
- Refresh fills the data sheets through the grid's reader, evaluates, and stores (the overlay only) or exports.
- The sheet room: join with the access check, the workbook loaded and held, viewport requests answered with blocks, dropped when idle.
- Calc mode in `Grid.xml` and `Grid.qvue`: the sheet component drawing the blocks, sheet tabs, Chart.js charts, the document in the view list, export as XLSX. An uploaded XLSX opens as a workbook. No editing yet.

### Phase 2b: CalcGrid in Dashboards

- Access profiles in the engine: the profile artifact, the offered entities, the room keyed by scope, `check#CalcProfile`, each call checked as its caller.
- The `CALC_GRID` group and the `admin` profile (tools); the `sales` profile (mr-mantle) and its grants (mr-boss), with the regression specs of each: a sales user of one organization reads its orders and none of another's, in the sheet, a summary, a chart and an export; an entity outside the profile is refused; a user with no organization reads nothing.
- `Dashboards/CalcGrid.xml`: the list, a document read only, the source picker from the profile, sharing as a dashboard is shared, and the screen under a mount; the BOSS Insights mount.
- The `calcgrid` dashboard panel, and scheduled export through the existing report schedule, run as its owner.
- Guide: a CalcGrid section in [Monitoring](../docs/tools/Monitoring.md) and the Calc mode in [Database](../docs/tools/Database.md); the profiles in [Attack-Surface](../docs/security/Attack-Surface.md).

### Phase 3: editing

Both screens get it at once, since the sheet component is one.

- Formula bar, cell editor, reference picking, autocomplete from `formula_completion`.
- Styles, number formats, widths, heights, freeze, conditional formatting.
- Undo and redo, copy and paste, fill handle.
- Edits applied in the room in the order received, each viewer sent its viewport again; presence and other users' selections from awareness frames.
- Overlay edits relayed to the other rooms of the document (other scopes), or one editing room at a time, by the outcome of Phase 0.
- In a cluster, the owning server for an open workbook.
- Merged cells shown through an overlay; not editable in the first version.
- A terminal view in `moqui-tui`, with TironCalc as the reference, if wanted.

### Phase 4: charts and images in XLSX

The first place the engine itself needs to grow.

- **Images:** an image list on the worksheet, `xl/media` files, a drawing part for each sheet with cell anchors and its relationships, content types, the `<drawing>` element. A few hundred lines.
- **Charts:** the same drawing parts, a chart model and a chart part for each chart. A business subset (column, bar, line, area, pie, scatter; titles, legend, axes, number formats, series colours) is perhaps 1,500 to 2,500 lines, with rust_xlsxwriter's `chart.rs` as the reference for the XML.
- **Import** of both is about the same work again. Without it a workbook opened with charts loses them on save, as now.
- The model structs are what `to_bytes` stores, so this changes the stored form. Agree the chart and image model with upstream first; fork only if that fails or takes too long.
- The way round a fork: keep rust_xlsxwriter for exports with charts and walk the IronCalc model into it. It means mapping every style by hand and keeping two writers, so it is the fallback.
- rust_xlsxwriter is dropped when this phase is done.

### Phase 5: as needed

- The engine in the browser as WASM, fed by the diff queue, if the round trip is too slow; and offline editing by merge, on IronCalc's CRDT layer when it exists.
- An undo stack for each user.
- Pivot tables, when upstream has them or the summary sheet proves too little.
- A report query over a search dataset as a data sheet, under the dataset's policy.
- More access profiles, in the applications that own the entities.
- Formulas that read entities directly, which needs a custom function hook in the engine.
- Opening xls, xlsb and ods for editing through calamine.
- Incremental recalculation, if the row and formula limits get in the way.

## Decisions

Settled 2026-11-13.

| Decision | Outcome |
|---|---|
| Where the viewer and editor are | In the existing Tools › Data › Grid screen, as a Calc mode beside Records; no separate Tools screen |
| The limited version | CalcGrid in the Dashboards app, the same component, for relational data beside the search-based report tools |
| How it is limited | By authorization data: access profiles (an artifact group, its grants, their entity filter sets), never by the document |
| First profiles | `admin`, as the Tools app; `sales`, the rows of the user's associated organizations |
| Sources in CalcGrid | Entities and views through the Entity Facade; no SQL; business datasets (search) not a source at first |
| Summaries | A grouped view through the Entity Facade, not search aggregations |
| Where `sales` lives | The artifact group and its entity list in mr-mantle, beside the filter sets; the grants to the BOSS sales groups in mr-boss; CalcGrid shown in BOSS under Insights by its mount |
| Editing in CalcGrid | The profile's grant action: view opens documents shared with the user, with refresh and export; update also authors documents (overlay, sources from the profile, charts). Data sheets are read only for everyone |
| The document's store | A report definition of type `calcgrid` (`moqui.report.Definition`), the overlay bytes in a row beside it: sharing with groups, versions, stars, git sync and schedules as dashboards have. The Tools grid lists the same documents |
| Release | Phases 0 to 3 (through editing, in both screens) are for v0.1.0, if the spike holds; charts and images in XLSX (Phase 4) and Phase 5 after |

## Open questions

One, for Phase 0 to answer: **rooms of different scopes while editing.** Overlay edits are relayed to the other rooms of a document through the diff queue if the spike shows it works, else a document has one editing room at a time and the others follow it on reload.

## Working with upstream

No fork first. A fork only for what cannot wait, kept small and offered back.

- **IronCalc.** Small requests that make our use easier: sheet protection, a list of changed cells after an evaluation (it would let the room send changes in place of whole viewports), selection and undo kept outside the model for several users, a hook for custom functions, error types. Larger: the chart and image model. They plan charts after 1.0, so our work there is either a contribution or a divergence; talk before writing it.
- **Tabulator.** Extension points only. If merged cells or reference picking need a hook that is missing, propose the hook.
- **Versions.** Pin IronCalc to a release and move on purpose; the API will change before 1.0.

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
