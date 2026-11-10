# IronCalc DB: database-driven spreadsheets

Proposal, 2026-11-09. No code in the workspace. Facts about other projects come from reading their source and docs at that date; nothing here was built or run, and the items marked "to verify" need a spike before any phase is committed to. The engine runs in the server and the browser is a thin client ([Thin client](#thin-client)).

## The idea

A spreadsheet for business users whose data comes from the database, not from a file:

- **A data sheet** filled from the Entity Facade: an entity or view-entity with query conditions, or a business dataset with its row-filter policy. The query defines which records are in the sheet; filtering and sorting are changes to the query.
- **An overlay document** on top of that data: more sheets with formulas, formatting, conditional formatting, summaries and charts. The overlay is saved; the data is refilled when the document is opened or refreshed.
- **Files in and out**: open an XLSX (and through calamine the older formats), save or export the result as XLSX with its charts and images, and run the same document on a schedule.

It sits beside the Tools data grid (Tabulator, for administrators working on records) as the analytics view of the same data, and it gives the applications one spreadsheet component for user-authored sheets.

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

The grid stays Tabulator, which is already vendored (6.7.0) and already drives [`Grid.qvue`](../../runtime/base-component/tools/screen/Tools/Data/Grid.qvue) with range selection, range copy and paste, undo, frozen columns and remote loading. IronCalc's own web widget is React with a canvas grid and is not used.

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
 (entity or view-entity, conditions,           │ set_row
  or a business dataset and its row filter)    ▼
                                        ┌──────────────┐
 overlay document (stored bytes) ─────► │ IronCalc     │ ──► XLSX, scheduled export
                                        │ model, one   │
 search aggregations ──► summary sheet ►│ for each open│
                                        │ document     │
                                        └──────┬───────┘
                          edits, viewport ▲    │ blocks of cells, sheet layout, presence
                          (WebSocket room)│    ▼
                              Tabulator view, formula bar, Chart.js charts
                              moqui-tui view
```

- **The data sheet** is read-only and declared as an Excel table, so overlay formulas name its columns and survive a changing row count. Security is applied where the rows are read: the artifact authorization of the entity and the dataset's row filter.
- **Row limit.** Everything is in server memory and recalculated in full, so a data sheet has a cap, as the XLSX download has now (100,000 rows).
- **Summaries.** Grouped totals come from the search aggregations, placed on a second data sheet. That stands in for pivot tables and keeps the heavy work in the database.
- **Charts.** A chart is a small definition in the overlay that names ranges. The server sends the values of those ranges; the browser draws them with the Chart.js already vendored; the export writes a native Excel chart.

## Thin client

The engine and the workbook are in the server only. The browser gets the cells on screen and sends edits. From the source this looks like the better first design, ahead of a copy of the engine in the browser:

**What makes it work**

- **`get_cells_block`** (above) was added in October 2026 for IronCalc's own canvas, to replace a call for each cell. It is the read a remote view needs. With it go the row and column sizes, merged cells, frozen counts and links, each one call. A screen of 60 rows by 30 columns is 1,800 style indices, a short style list and the non-empty texts: an estimate of a few tens of kilobytes of JSON before compression.
- **The room is already built.** `moqui-collab` keeps a document for each co-edited record in memory, on the notification WebSocket: `join` (the access check and the load), `apply` (an edit), `tick` (store a while after an edit, drop when the last editor leaves), `save`, and `awareness` frames for presence. A sheet room is the same shape with a workbook in place of a Yjs document.
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

Each phase leaves something usable. The first three need no fork of IronCalc and no change to Tabulator.

### Phase 0: spike

Answer the items marked "to verify" and the ones below before committing to the rest.

- Build `ironcalc` into the host behind a feature; load 100,000 rows with `set_row`, add a few hundred formulas, time an evaluation and a save.
- Round-trip a set of real workbooks; list what is lost.
- Structured references to a table whose row count changes on refresh.
- A `Model<'static>` held in a room behind a lock: does it compile as `Send`, and what do an edit, an evaluation and a `get_cells_block` of one screen cost at that size.
- A `.qvue` screen with Tabulator fed by blocks over the notification WebSocket: a formatter that draws from the block, an editor that sends the edit, the viewport redrawn from the answer. Measure the round trip, and try formula completion through it.
- Loading by viewport in Tabulator, including a jump to a far row.
- The ESM build of Tabulator in `webroot/libs` and a first custom module.

### Phase 1: the engine in the server

- An `ironcalc` feature beside `xlsx`.
- The xlsx render mode, `SpreadsheetServices` and scheduled reports write through the IronCalc model. Sheet protection is the one missing piece: ask upstream for it, and keep rust_xlsxwriter for the protected exports until it lands.
- A service that opens an XLSX, sets cells, evaluates and returns values or a file: calculation over a template workbook, useful on its own.
- calamine unchanged.

### Phase 2: the virtual sheet, read and export

- The document: a query definition, the overlay bytes, chart definitions. Entities and services for create, open, refresh, save and export.
- Refresh fills the data sheet and the summary sheet, evaluates, and stores or exports.
- The sheet room: join with the access check, the workbook loaded and held, viewport requests answered with blocks, dropped when idle.
- A viewing screen: Tabulator drawing the blocks, sheet tabs, Chart.js charts. No editing yet.
- Scheduled export through the existing report schedule.

### Phase 3: editing

- Formula bar, cell editor, reference picking, autocomplete from `formula_completion`.
- Styles, number formats, widths, heights, freeze, conditional formatting.
- Undo and redo, copy and paste, fill handle.
- Edits applied in the room in the order received, each viewer sent its viewport again; presence and other users' selections from awareness frames.
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
- Formulas that read entities directly, which needs a custom function hook in the engine.
- Opening xls, xlsb and ods for editing through calamine.
- Incremental recalculation, if the row and formula limits get in the way.

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
- **Fork drift** if Phase 4 is not taken upstream.
- **Expectations.** It will not be Excel. Say which features are in before users find which are not.
