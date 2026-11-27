# MR Moqui — UI review: Tools

**Active pass · November 26.** [TOOL-B04](#tool-b04--november-26-change-led-review) records the selected branches and their outcomes. Earlier batches retain their dated evidence.

**Review update — 2026-11-25.** TOOL-B03 records the latest live evidence. The activity ledger identifies the branches still open.

**Status — 2026-11-25.** All four Grid findings are fixed and checked live. Back to Find from Record now reopens the Grid with its filter, and one result reads “1 row” (TOOL-UI-003/004).

**Original review — 2026-11-16:** review pass in progress. B01 exercised bounded Home → Grid → Record and SQL
read/explain/error/history branches in Material light; two findings await consolidated triage. Editing, schema,
access/refusal branches and other modes remain pending. No fixes or tests performed.

Contents: [scope](#1-purpose-scope-and-sources) · [method](#2-review-method-and-refinement-criteria) ·
[inventory](#3-screen-and-surface-inventory) · [activities](#4-end-to-end-activities) ·
[verification](#5-existing-checks-and-future-verification) · [record](#6-coverage-findings-and-running-record) ·
[decisions](#7-decisions).

## 1. Purpose, scope and sources

Review Tools as the data/service operator's workspace: choose the right database and artifact, inspect or change a
bounded set of data, understand the proposed operation, follow its job and recover failed items. Technical terms are
appropriate here, but source identity, scope, confirmation and outcomes still need to be clear.

Sources: [tools README](../runtime/components/base/tools/README.md), [Database](../docs/tools/Database.md),
[Load Runner](../docs/tools/Load-Runner.md), [database design](archive/Tools-Database.md),
[load runner record](archive/LoadRunner.md), [IronCalc](IronCalc-DB.md), current `tools/screen/Tools`,
`DatabaseParts.xml`, `ToolsRecent.js`, `LoadRuns.js`, and framework database/grid/ETL/load services.
Use the [archived review](archive/UI-Refinement.md) for unresolved workflow coverage, not current pass status.

### Ownership and handoffs

| Surface | Owner / handoff |
| --- | --- |
| Tools root and all Data/SQL/Schema/ETL/Database/Service/Load screens | This plan |
| Calc document sharing, profiles, collaborative editing and reporting | [Dashboards](ui-review-dashboards.md); Tools owns acquisition of its broader grid/file/SQL sources |
| Registered imports/exports/backups, job administration, audit | [System](ui-review-system.md); Tools owns generic database/data operations |
| Telemetry embeds in service/entity/database details | [Monitor](ui-review-system-monitoring.md); retain Tools context and return path |
| Edit source/model/service from a selected artifact | [Dev AIDE authoring](ui-review-dev-aide-authoring.md); Tools only follows the supported link and return |

SQL, maintenance, load tests and data imports are deliberate operations on disposable data. This plan is not a
performance benchmark or a schema migration project.

## 2. Review method and refinement criteria

Apply the [common review protocol](ui-review-shared-gallery.md#common-review-protocol) and
[verification policy](ui-review-shared-gallery.md#verification-policy-for-later-execution). The details below add this
area's fixtures, dependencies and judgment criteria.

### Environment, identities and records

- Prepare `review.tools.operator` with Tools access, `sec.view.only` and `sec.none.only`. Add only the relevant
  positive grant for `SQL_RUNNER_WEB`, `DATABASE_ADMIN`, `DATABASE_EXTERNAL` or `LOAD_RUNNER`; all are distinct from
  basic app visibility. Use missing-permission counterparts with the same valid parameters.
- Select a disposable `group:<groupName>` database and, for external connection coverage, a local disposable
  `ext:<connectionId>` on a dev instance. Check selected database persistence across Data, SQL, Schema, ETL and
  Database, including the deliberate boundary: Schema, entity sources and ETL targets offer entity-group databases,
  not external ones. Returning there from an external SQL source must show the actual supported selection.
- Data fixtures: small parent/child framework entities, multiple pages, lookup labels outside page one, saved grid
  views, a DbViewEntity, DataDocument and feed, valid/mixed/invalid CSV/XLSX/XML files, snapshot archive, SQL query,
  schema difference, harmless maintenance job and ETL mapping/value map. Each has a review-specific identity.
- Use a short safe local service and small local load plan; never target an existing development/production server.
  Source files, result exports, snapshots and external credentials stay in the disposable runtime. Keep fixture
  databases/snapshots separate from the review session's own mutable state; sequence DDL/load/maintenance operations.
- Refused database type/capability and non-dev external access are explicit branches. No installed external database,
  spreadsheet engine or load capability means its success path is blocked, not silently omitted.

### Area-specific criteria

- The database/source/entity identity stays visible through navigation, dialogs, jobs and output. A remembered
  database cannot silently retarget a pending action.
- Grid optimistic edits, saved views, database writes and Calc overlays are distinguishable. Validation names rows
  and fields and preserves pending edits; save completion is tied to the submitted revision.
- Read-only SQL and confirmed writes explain scope and result; cancellation/timeout cannot be labelled rollback
  without evidence. Schema planning, queued work and applied DDL are distinct stages.
- Import/ETL errors are per-item where independent; successful rows survive failure/retry. Exports have truthful
  counts/columns and cannot become another user's file. Load test progress distinguishes ramp, running, stopping,
  stopped and failed and keeps raw results available for comparison.

### Ordered waves

| Wave | Activities | Continuation |
| --- | --- | --- |
| 1. Discover and inspect data | TOOL-01–06 | Artifact/source → grid/record → view/document |
| 2. Move and transform data | TOOL-07–10, TOOL-14 | Import/export/snapshot/SQL → ETL |
| 3. Maintain and invoke | TOOL-11–13, TOOL-15 | Schema/database job → safe service |
| 4. Exercise load and reconcile | TOOL-16–17 | Short local run/comparison and remaining access/modes |

Save baseline fixtures before mutation; do not restore a snapshot over another activity's database. Keep a stable
entity/view/document until its export and ETL consumers finish. DDL and active-session cancellation are serialized.

## 3. Screen and surface inventory

This inventory accounts for **64 XML definitions** assigned to this file; screen containers and helper
definitions are included. Routes below are Material paths unless labelled otherwise; app routes have Standard
`/apps` counterparts. A file's physical presence does not prove access or a supported interaction in every mode.
Each row maps to §4; helper inspection does not replace its parent workflow. Counts describe definitions, not user
activities. Dialogs, templates and non-XML surfaces follow the route tables.

### tools: `Tools`

| Route / surface | XML source | Default / variants / boundary | Activities |
| --- | --- | --- | --- |
| `/qapps/tools/ArtifactStats` | [ArtifactStats.xml](../runtime/components/base/tools/screen/Tools/ArtifactStats.xml) | Inspect the route and its visible controls through the activity. | TOOL-01, TOOL-15 |
| `/qapps/tools/Data` | [Data.xml](../runtime/components/base/tools/screen/Tools/Data.xml) | Default: `Grid`. | TOOL-01, TOOL-02 |
| `/qapps/tools/Database` | [Database.xml](../runtime/components/base/tools/screen/Tools/Database.xml) | Default: `Overview`. | TOOL-12 |
| `/qapps/tools/Etl` | [Etl.xml](../runtime/components/base/tools/screen/Tools/Etl.xml) | Default: `Mappings`. | TOOL-14 |
| `/qapps/tools/Home` | [Home.xml](../runtime/components/base/tools/screen/Tools/Home.xml) | Authored text: `qvt`, `tui`. Context: `db`. | TOOL-01 |
| `/qapps/tools/Load` | [Load.xml](../runtime/components/base/tools/screen/Tools/Load.xml) | Default: `Scripts`. | TOOL-16 |
| `/qapps/tools/RestApi` | [RestApi.xml](../runtime/components/base/tools/screen/Tools/RestApi.xml) | Context: `filter`, `api`. | TOOL-15 |
| `/qapps/tools/Schema` | [Schema.xml](../runtime/components/base/tools/screen/Tools/Schema.xml) | Default: `Entities`. | TOOL-11 |
| `/qapps/tools/Service` | [Service.xml](../runtime/components/base/tools/screen/Tools/Service.xml) | Default: `ServiceReference`. | TOOL-15 |
| `/qapps/tools/Sql` | [Sql.xml](../runtime/components/base/tools/screen/Tools/Sql.xml) | Companions: [Sql.tui.json](../runtime/components/base/tools/screen/Tools/Sql.tui.json), [Sql.qvue](../runtime/components/base/tools/screen/Tools/Sql.qvue). Authored text: `qvue`, `tui`. Declared modes: `vue,qvue,tui`. | TOOL-10 |
| `/qapps/tools/SqlDownload` | [SqlDownload.xml](../runtime/components/base/tools/screen/Tools/SqlDownload.xml) | Authored text: `xlsx`. Hidden/helper entry; enter through its parent action and inspect direct access. Declared modes: `xlsx`. | TOOL-10 |
| `/qapps/tools/SqlTui` | [SqlTui.xml](../runtime/components/base/tools/screen/Tools/SqlTui.xml) | Authored text: `html,vuet,qvt`, `tui`. Hidden/helper entry; enter through its parent action and inspect direct access. | TOOL-10 |
| `/qapps/tools/StatusFlows` | [StatusFlows.xml](../runtime/components/base/tools/screen/Tools/StatusFlows.xml) | Authored text: `html,vuet`, `qvt`, `tui`. | TOOL-15 |

### tools: `Tools/Data`

| Route / surface | XML source | Default / variants / boundary | Activities |
| --- | --- | --- | --- |
| `/qapps/tools/Data/Browse` | [Browse.xml](../runtime/components/base/tools/screen/Tools/Data/Browse.xml) | Default: `MainEntityList`. | TOOL-04 |
| `/qapps/tools/Data/Documents` | [Documents.xml](../runtime/components/base/tools/screen/Tools/Data/Documents.xml) | Default: `Index`. | TOOL-06 |
| `/qapps/tools/Data/Export` | [Export.xml](../runtime/components/base/tools/screen/Tools/Data/Export.xml) | Inspect the route and its visible controls through the activity. | TOOL-08 |
| `/qapps/tools/Data/Grid` | [Grid.xml](../runtime/components/base/tools/screen/Tools/Data/Grid.xml) | Companions: [Grid.qvue](../runtime/components/base/tools/screen/Tools/Data/Grid.qvue). Authored text: `qvue`, `tui`. Context: `entityName`, `selectedEntity`, `viewId`. Declared modes: `vue,qvue,tui`. | TOOL-02, TOOL-03 |
| `/qapps/tools/Data/Load` | [Load.xml](../runtime/components/base/tools/screen/Tools/Data/Load.xml) | Inspect the route and its visible controls through the activity. | TOOL-07 |
| `/qapps/tools/Data/Record` | [Record.xml](../runtime/components/base/tools/screen/Tools/Data/Record.xml) | Authored text: `qvt,html,html-print`, `tui`. Hidden/helper entry; enter through its parent action and inspect direct access. Context: `selectedEntity`. | TOOL-02, TOOL-04 |
| `/qapps/tools/Data/Snapshots` | [Snapshots.xml](../runtime/components/base/tools/screen/Tools/Data/Snapshots.xml) | Inspect the route and its visible controls through the activity. | TOOL-09 |
| `/qapps/tools/Data/Views` | [Views.xml](../runtime/components/base/tools/screen/Tools/Data/Views.xml) | Default: `FindDbView`. | TOOL-05 |

### tools: `Tools/Data/Browse`

| Route / surface | XML source | Default / variants / boundary | Activities |
| --- | --- | --- | --- |
| `/qapps/tools/Data/Browse/AutoEdit` | [AutoEdit.xml](../runtime/components/base/tools/screen/Tools/Data/Browse/AutoEdit.xml) | Authored text: `html`, `qvt`, `tui`. Context: `aen`. | TOOL-04 |
| `/qapps/tools/Data/Browse/AutoFind` | [AutoFind.xml](../runtime/components/base/tools/screen/Tools/Data/Browse/AutoFind.xml) | Context: `aen`. | TOOL-04 |
| `/qapps/tools/Data/Browse/MainEntityList` | [MainEntityList.xml](../runtime/components/base/tools/screen/Tools/Data/Browse/MainEntityList.xml) | Inspect the route and its visible controls through the activity. | TOOL-04 |

### tools: `Tools/Data/Browse/AutoEdit`

| Route / surface | XML source | Default / variants / boundary | Activities |
| --- | --- | --- | --- |
| `/qapps/tools/Data/Browse/AutoEdit/AutoEditDetail` | [AutoEditDetail.xml](../runtime/components/base/tools/screen/Tools/Data/Browse/AutoEdit/AutoEditDetail.xml) | Context: `aen`, `den`. | TOOL-04 |
| `/qapps/tools/Data/Browse/AutoEdit/AutoEditMaster` | [AutoEditMaster.xml](../runtime/components/base/tools/screen/Tools/Data/Browse/AutoEdit/AutoEditMaster.xml) | Context: `aen`, `den`. | TOOL-04 |

### tools: `Tools/Data/Documents`

| Route / surface | XML source | Default / variants / boundary | Activities |
| --- | --- | --- | --- |
| `/qapps/tools/Data/Documents/Edit` | [Edit.xml](../runtime/components/base/tools/screen/Tools/Data/Documents/Edit.xml) | Default: `FindDataDocument`. | TOOL-06 |
| `/qapps/tools/Data/Documents/Index` | [Index.xml](../runtime/components/base/tools/screen/Tools/Data/Documents/Index.xml) | Inspect the route and its visible controls through the activity. | TOOL-06 |
| `/qapps/tools/Data/Documents/ViewDocument` | [ViewDocument.xml](../runtime/components/base/tools/screen/Tools/Data/Documents/ViewDocument.xml) | Hidden/helper entry; enter through its parent action and inspect direct access. Context: `_index`, `_id`. | TOOL-06 |

### tools: `Tools/Data/Documents/Edit`

| Route / surface | XML source | Default / variants / boundary | Activities |
| --- | --- | --- | --- |
| `/qapps/tools/Data/Documents/Edit/DataDocFields` | [DataDocFields.xml](../runtime/components/base/tools/screen/Tools/Data/Documents/Edit/DataDocFields.xml) | Context: `dataDocumentId`. | TOOL-06 |
| `/qapps/tools/Data/Documents/Edit/EditDataDocument` | [EditDataDocument.xml](../runtime/components/base/tools/screen/Tools/Data/Documents/Edit/EditDataDocument.xml) | Context: `dataDocumentId`. | TOOL-06 |
| `/qapps/tools/Data/Documents/Edit/FindDataDocument` | [FindDataDocument.xml](../runtime/components/base/tools/screen/Tools/Data/Documents/Edit/FindDataDocument.xml) | Hidden/helper entry; enter through its parent action and inspect direct access. | TOOL-06 |
| `/qapps/tools/Data/Documents/Edit/ViewDataDocument` | [ViewDataDocument.xml](../runtime/components/base/tools/screen/Tools/Data/Documents/Edit/ViewDataDocument.xml) | Context: `dataDocumentId`. | TOOL-06 |

### tools: `Tools/Data/Documents/Edit/DataDocFields`

| Route / surface | XML source | Default / variants / boundary | Activities |
| --- | --- | --- | --- |
| `/qapps/tools/Data/Documents/Edit/DataDocFields/FieldList` | [FieldList.xml](../runtime/components/base/tools/screen/Tools/Data/Documents/Edit/DataDocFields/FieldList.xml) | Hidden/helper entry; enter through its parent action and inspect direct access. Context: `dataDocumentId`. | TOOL-06 |

### tools: `Tools/Data/Views`

| Route / surface | XML source | Default / variants / boundary | Activities |
| --- | --- | --- | --- |
| `/qapps/tools/Data/Views/EditDbView` | [EditDbView.xml](../runtime/components/base/tools/screen/Tools/Data/Views/EditDbView.xml) | Context: `dbViewEntityName`, `dbViewEntityName_0`. | TOOL-05 |
| `/qapps/tools/Data/Views/FindDbView` | [FindDbView.xml](../runtime/components/base/tools/screen/Tools/Data/Views/FindDbView.xml) | Inspect the route and its visible controls through the activity. | TOOL-05 |
| `/qapps/tools/Data/Views/ViewDbView` | [ViewDbView.xml](../runtime/components/base/tools/screen/Tools/Data/Views/ViewDbView.xml) | Context: `dbViewEntityName`. | TOOL-05 |

### tools: `Tools/Database`

| Route / surface | XML source | Default / variants / boundary | Activities |
| --- | --- | --- | --- |
| `/qapps/tools/Database/Activity` | [Activity.xml](../runtime/components/base/tools/screen/Tools/Database/Activity.xml) | Inspect the route and its visible controls through the activity. | TOOL-12 |
| `/qapps/tools/Database/Connections` | [Connections.xml](../runtime/components/base/tools/screen/Tools/Database/Connections.xml) | Hidden/helper entry; enter through its parent action and inspect direct access. | TOOL-13 |
| `/qapps/tools/Database/JobDetail` | [JobDetail.xml](../runtime/components/base/tools/screen/Tools/Database/JobDetail.xml) | Companions: [JobDetail.tui.json](../runtime/components/base/tools/screen/Tools/Database/JobDetail.tui.json), [JobDetail.qvue](../runtime/components/base/tools/screen/Tools/Database/JobDetail.qvue). Authored text: `qvue`, `tui`. Hidden/helper entry; enter through its parent action and inspect direct access. Context: `jobId`. Declared modes: `vue,qvue,tui`. | TOOL-13 |
| `/qapps/tools/Database/JobDetailTui` | [JobDetailTui.xml](../runtime/components/base/tools/screen/Tools/Database/JobDetailTui.xml) | Hidden/helper entry; enter through its parent action and inspect direct access. Context: `jobId`. | TOOL-13 |
| `/qapps/tools/Database/Jobs` | [Jobs.xml](../runtime/components/base/tools/screen/Tools/Database/Jobs.xml) | Inspect the route and its visible controls through the activity. | TOOL-13 |
| `/qapps/tools/Database/Overview` | [Overview.xml](../runtime/components/base/tools/screen/Tools/Database/Overview.xml) | Inspect the route and its visible controls through the activity. | TOOL-12 |
| `/qapps/tools/Database/Queries` | [Queries.xml](../runtime/components/base/tools/screen/Tools/Database/Queries.xml) | Inspect the route and its visible controls through the activity. | TOOL-12, TOOL-13 |
| `/qapps/tools/Database/Settings` | [Settings.xml](../runtime/components/base/tools/screen/Tools/Database/Settings.xml) | Inspect the route and its visible controls through the activity. | TOOL-12 |
| `/qapps/tools/Database/SpeedTest` | [SpeedTest.xml](../runtime/components/base/tools/screen/Tools/Database/SpeedTest.xml) | Inspect the route and its visible controls through the activity. | TOOL-13 |
| `/qapps/tools/Database/Tables` | [Tables.xml](../runtime/components/base/tools/screen/Tools/Database/Tables.xml) | Inspect the route and its visible controls through the activity. | TOOL-12, TOOL-13 |

### tools: `Tools/Etl`

| Route / surface | XML source | Default / variants / boundary | Activities |
| --- | --- | --- | --- |
| `/qapps/tools/Etl/Mapping` | [Mapping.xml](../runtime/components/base/tools/screen/Tools/Etl/Mapping.xml) | Companions: [Mapping.qvue](../runtime/components/base/tools/screen/Tools/Etl/Mapping.qvue). Authored text: `qvue`, `tui`. Hidden/helper entry; enter through its parent action and inspect direct access. Context: `mappingId`. Declared modes: `vue,qvue,tui`. | TOOL-14 |
| `/qapps/tools/Etl/Mappings` | [Mappings.xml](../runtime/components/base/tools/screen/Tools/Etl/Mappings.xml) | Context: `term`. | TOOL-14 |
| `/qapps/tools/Etl/Run` | [Run.xml](../runtime/components/base/tools/screen/Tools/Etl/Run.xml) | Hidden/helper entry; enter through its parent action and inspect direct access. Context: `runId`. | TOOL-14 |
| `/qapps/tools/Etl/Runs` | [Runs.xml](../runtime/components/base/tools/screen/Tools/Etl/Runs.xml) | Context: `mappingId`. | TOOL-14 |
| `/qapps/tools/Etl/ValueMaps` | [ValueMaps.xml](../runtime/components/base/tools/screen/Tools/Etl/ValueMaps.xml) | Inspect the route and its visible controls through the activity. | TOOL-14 |

### tools: `Tools/Load`

| Route / surface | XML source | Default / variants / boundary | Activities |
| --- | --- | --- | --- |
| `/qapps/tools/Load/Compare` | [Compare.xml](../runtime/components/base/tools/screen/Tools/Load/Compare.xml) | Hidden/helper entry; enter through its parent action and inspect direct access. Context: `runIds`. | TOOL-16 |
| `/qapps/tools/Load/NewRun` | [NewRun.xml](../runtime/components/base/tools/screen/Tools/Load/NewRun.xml) | Companions: [NewRun.qvue](../runtime/components/base/tools/screen/Tools/Load/NewRun.qvue), [NewRun.tui.json](../runtime/components/base/tools/screen/Tools/Load/NewRun.tui.json). Authored text: `qvue`, `tui`. Hidden/helper entry; enter through its parent action and inspect direct access. Context: `scriptLocation`, `runId`. Declared modes: `vue,qvue,tui`. | TOOL-16 |
| `/qapps/tools/Load/Run` | [Run.xml](../runtime/components/base/tools/screen/Tools/Load/Run.xml) | Companions: [Run.qvue](../runtime/components/base/tools/screen/Tools/Load/Run.qvue), [Run.tui.json](../runtime/components/base/tools/screen/Tools/Load/Run.tui.json). Authored text: `qvue`, `tui`. Hidden/helper entry; enter through its parent action and inspect direct access. Context: `runId`. Declared modes: `vue,qvue,tui`. | TOOL-16 |
| `/qapps/tools/Load/Runs` | [Runs.xml](../runtime/components/base/tools/screen/Tools/Load/Runs.xml) | Authored text: `html`, `qvt`, `tui`. Context: `scriptLocation`, `status`, `startedFrom`. | TOOL-16 |
| `/qapps/tools/Load/Scripts` | [Scripts.xml](../runtime/components/base/tools/screen/Tools/Load/Scripts.xml) | Inspect the route and its visible controls through the activity. | TOOL-16 |

### tools: `Tools/Schema`

| Route / surface | XML source | Default / variants / boundary | Activities |
| --- | --- | --- | --- |
| `/qapps/tools/Schema/Entities` | [Entities.xml](../runtime/components/base/tools/screen/Tools/Schema/Entities.xml) | Inspect the route and its visible controls through the activity. | TOOL-11 |
| `/qapps/tools/Schema/Entity` | [Entity.xml](../runtime/components/base/tools/screen/Tools/Schema/Entity.xml) | Hidden/helper entry; enter through its parent action and inspect direct access. Context: `selectedEntity`, `refresh`. | TOOL-11 |
| `/qapps/tools/Schema/IndexesKeys` | [IndexesKeys.xml](../runtime/components/base/tools/screen/Tools/Schema/IndexesKeys.xml) | Inspect the route and its visible controls through the activity. | TOOL-11 |

### tools: `Tools/Service`

| Route / surface | XML source | Default / variants / boundary | Activities |
| --- | --- | --- | --- |
| `/qapps/tools/Service/ServiceDetail` | [ServiceDetail.xml](../runtime/components/base/tools/screen/Tools/Service/ServiceDetail.xml) | Hidden/helper entry; enter through its parent action and inspect direct access. Context: `serviceName`. | TOOL-15 |
| `/qapps/tools/Service/ServiceReference` | [ServiceReference.xml](../runtime/components/base/tools/screen/Tools/Service/ServiceReference.xml) | Inspect the route and its visible controls through the activity. | TOOL-15 |
| `/qapps/tools/Service/ServiceRun` | [ServiceRun.xml](../runtime/components/base/tools/screen/Tools/Service/ServiceRun.xml) | Context: `serviceName`. | TOOL-15 |

### tools: `roots`

| Route / surface | XML source | Default / variants / boundary | Activities |
| --- | --- | --- | --- |
| `/qapps/tools` | [Tools.xml](../runtime/components/base/tools/screen/Tools.xml) | Default: `Home`. | TOOL-01, TOOL-17 |

### Embedded surfaces, dialogs, outputs and reconciliation

| Supporting surface | Activity / inspection |
| --- | --- |
| Home jump pickers, recent entities/views/queries, running/recent job cards and database picker | TOOL-01/12; visibility and supported capability differ |
| Grid QVue source chooser, lookup, paste/range edits, undo, pending changes, saved views, export; Calc and file Load into… | TOOL-02/03/14; dialogs are within Grid, not invented routes |
| Browse dynamic AutoFind/AutoEdit master/detail and getFormXml helpers | TOOL-04; `aen`/`den` and record keys must follow the correct master/detail |
| Views field selection/CSV; DataDocument edit fields/conditions/groups/feeds/links and indexed document viewer | TOOL-05/06; include nested hidden routes and actual output |
| Data Load Check Data/difference selection, synchronous create-only/create-or-update and optional background wrapper; entity/document Export; snapshot upload/import/download/delete | TOOL-07–09; distinguish selected field writes, normal loads and each job's actual outcome |
| [DatabaseParts.xml](../runtime/components/base/tools/template/DatabaseParts.xml): DatabasePicker/selectDatabase and DroppedKeys sections | TOOL-01/07/09/11–13/17; shared database context and unresolved foreign-key restoration evidence |
| SQL QVue catalog, classify/confirm, result grid/explain, history/saved dialogs, SqlTui and SqlDownload | TOOL-10; saved query opens without executing |
| Schema Check Tables and IndexesKeys plan/download DDL; Database JobDetail QVue/twin | TOOL-11/13; proposed SQL, progress, cancel and outcome |
| ETL Mapping QVue source/target/field match/sample/error/value map/schedule controls; root JSON transitions | TOOL-14; file upload and parent transition context |
| Load NewRun/Run QVue with `.tui.json` companions; stop versus stop-now, user adjustment, download/import/compare | TOOL-16; no unattended stress target |
| RestApi JSON documents and ArtifactStats/StatusFlows diagrams | TOOL-15; RestApi links descriptions; it is not an interactive API viewer |

GroovyShell is removed and is not reinstated. JavaScript execution belongs to the shared Terminal/Dev AIDE where
currently offered. Older raw data routes are reconciled to the current Data/Schema layout; the legacy Browse tree
still present in source stays in scope.

## 4. End-to-end activities

Use visible controls and inspect intermediate states, confirmation and the resulting data/output. Unless narrowed
below, each activity has Material light/dark, actual supported TUI and a brief Standard/output branch. Select narrow
and keyboard checks where layout/interaction differs. Record each unattempted branch in §6; a service call or render
alone does not complete the activity. Names beginning `review.` are fixture proposals, not shipped accounts.

### TOOL-01 — Choose the database and resume an artifact

- **Actor/start:** Tools operator and view-only counterpart; two disposable database choices and recent
  artifacts.
- **Objective/steps:** Home → database picker → entity/service search → open → return → recent view/query →
  running/recent job; revisit after reload.
- **Expected:** Selected database and artifact are clear; recent SQL opens without executing; missing
  permissions explain unavailable tools without hiding ordinary navigation.
- **Recovery:** No recent work, unknown entity/service, removed view, inaccessible database and failed count
  leave useful independent controls.
- **Screens/modes:** Home, Data/Schema/Service/Database roots and ArtifactStats; browser/TUI/Standard and
  keyboard search.

### TOOL-02 — Edit and save a bounded grid

- **Actor/start:** Operator with parent/lookup fixture records, multiple pages and a read-only counterpart.
- **Objective/steps:** Data/Grid → find/sort/page → lookup value → edit/paste range → undo → inspect pending
  changes → save → reopen record/view → export; save/reload named view.
- **Expected:** Row keys, selected labels and source remain stable; saved data matches acknowledged edits; view
  preferences and data writes are distinct; exported columns/values match selection.
- **Recovery:** Invalid cell, stale row, failed save, canceled source change, lookup not on first page and late
  response preserve recoverable edits.
- **Screens/modes:** Grid QVue and Record handoff; browser keyboard/narrow, actual TUI row editor and Standard
  supported fallback.

### TOOL-03 — Open a file or result as a grid and spreadsheet

- **Actor/start:** Operator with small CSV/XLSX files and known SQL/view/document source; separate Calc
  document.
- **Objective/steps:** Grid source chooser → upload/open file → inspect columns/types → Calc mode →
  formula/format → save/export document → return to source; follow file Load into… only to ETL setup.
- **Expected:** File/grid/source identity is clear; Calc overlay does not silently update source data; workbook
  results survive save/reopen and match exported bytes.
- **Recovery:** Malformed file, unsupported type, empty sheet, invalid formula, failed upload and canceled
  source switch retain prior work without claiming import.
- **Screens/modes:** Grid QVue source/file/Calc dialogs; DASH-07/08 own profile/sharing/collaboration; actual
  XLSX and truthful TUI/Standard support.

### TOOL-04 — Browse and edit related records

- **Actor/start:** Operator and reader; disposable master with two different related detail records.
- **Objective/steps:** Data/Browse MainEntityList → AutoFind → selected AutoEditMaster → related AutoEditDetail
  → edit/create → return; compare Data/Record form for the same row.
- **Expected:** Master/detail keys and labels stay distinct; row actions open their own record; return retains
  the intended selection/filter.
- **Recovery:** Missing relationship, deleted row, validation refusal, canceled delete and denied field do not
  retarget another master/detail.
- **Screens/modes:** Browse/AutoFind/AutoEdit/Master/Detail, getFormXml helpers and Record; actual
  browser/TUI/Standard navigation.

### TOOL-05 — Define and reuse a database view

- **Actor/start:** Operator with disposable view name and related entity fields.
- **Objective/steps:** Data/Views FindDbView → create/EditDbView → select master/related fields → save →
  ViewDbView filter → CSV → open in Grid → reopen edit.
- **Expected:** Aliases, relationships and filters explain the rows; saved definition and CSV retain the
  selected columns and values.
- **Recovery:** Unknown field, invalid relationship, duplicate name, empty view and canceled change leave
  existing definition readable and input correctable.
- **Screens/modes:** Views/FindDbView/EditDbView/ViewDbView and Grid; field controls and actual CSV,
  TUI/Standard.

### TOOL-06 — Define, index and inspect a data document

- **Actor/start:** Operator with disposable DataDocument/feed and known source records.
- **Objective/steps:** Documents/Edit FindDataDocument → create/EditDataDocument → DataDocFields/FieldList add
  fields/conditions → configure groups/feed/link → ViewDataDocument → Index → ViewDocument → export.
- **Expected:** Definition fields and scope match indexed content; indexing status and document identity are
  explicit; independent indexing errors stay named.
- **Recovery:** Invalid field path, unauthorized group, missing feed/index, partial indexing and stale document
  explain the failing stage.
- **Screens/modes:** All Documents/Edit descendants, Index/ViewDocument and Export; nested fields/dialogs and
  actual document bytes.

### TOOL-07 — Load data and inspect per-file outcomes

- **Actor/start:** Load operator with safe XML/CSV files, one invalid independent file/row and copied database.
- **Objective/steps:** Data/Load → select data types/components, a local resource location, or XML/JSON/CSV text →
  Check Data → inspect differences and complete/incomplete keys → apply selected field updates/missing records →
  compare database values. Separately exercise Import Data — Create Only versus Create or Update, cancel one
  confirmation, then inspect the inline per-file messages and record readback. This screen has no file-upload
  chooser; use the named local resource or text inputs.
- **Worker branch:** Select Drop foreign keys first only on the scratch database with DATABASE_ADMIN. Follow the
  returned ServiceJob run and the separate database drop/restore jobs, including a failed load and restoration
  outcome. A normal import is synchronous; only the foreign-key wrapper takes this background path here.
- **Expected:** Check Data does not silently apply its differences; selected operations affect only the intended
  keys/fields. Create Only preserves existing rows. Source/destination and destructive options are clear;
  background request is not completion and independent failures retain named results.
- **Recovery:** Unreadable file, invalid record, failed DDL/foreign-key restoration, denied load and canceled
  confirmation retain precise failure evidence.
- **Screens/modes:** Data/Load, DatabaseLoadWrap job detail and Grid readback; no shared database,
  browser/TUI/Standard file delegation.

### TOOL-08 — Export exactly the intended records

- **Actor/start:** Operator and outsider; filtered entity and DataDocument fixture with multiple rows.
- **Objective/steps:** Data/Export → select entity/document/options → generate/download → inspect
  headers/counts/bytes → open through permitted consumer; compare direct DocumentExport.json behavior.
- **Expected:** Output type, filters and selected fields are clear; action-only columns absent; file write
  versus streamed output is distinguishable.
- **Recovery:** Empty result, invalid selection, refused location, failed export and another user download
  preserve scope and useful recovery.
- **Screens/modes:** Export, EntityExport/DocumentExport.json/exportDocuments and view/grid CSV; actual bytes,
  TUI/Standard output branch.

### TOOL-09 — Retain and reload a disposable snapshot

- **Actor/start:** Operator with independent scratch database and local archive location.
- **Objective/steps:** Snapshots → export → download/inspect → change isolated data → upload/import snapshot
  into the intended scratch target → inspect job/result → cancel then delete archive.
- **Expected:** Archive identity, data source, destination and overwrite effects are clear; restoration outcome
  is checked against fixture records, not a message alone.
- **Recovery:** Invalid archive, wrong target, missing permission, failed partial load and canceled delete keep
  the original archive and name unresolved results.
- **Screens/modes:** Data/Snapshots and background jobs/record readback; not System backup restore, local files
  and supported modes.
- **Archive branch:** Wait for ExportEntityDataSnapshot before inspecting the archive. Upload a same-named
  disposable ZIP and check replacement feedback; distinguish Upload from Import. Normal import uses
  ImportEntityDataSnapshot; the optional DATABASE_ADMIN foreign-key wrapper uses DatabaseLoadWrap and separate
  database jobs. Inspect import flags for ECAs, audit, feeds and foreign-key creation, plus the warning for a
  single-file snapshot whose wrapper cannot narrow the entity list. No ordinary snapshot import implies whole
  database replacement or a selectable external-database restore.

### TOOL-10 — Run, explain and recover a SQL query

- **Actor/start:** SQL_RUNNER_WEB holder and missing-permission counterpart; safe select and disposable write
  statement.
- **Objective/steps:** SQL → catalog/select → run read-only → inspect result/explain → save query → reopen
  history → edit write → inspect classification/confirmation → execute → cancel a harmless long fixture query →
  download result.
- **Expected:** Database, statement, parameters and result correspond; saved/history opens do not auto-run;
  write confirmation names effects; cancel state is accurate.
- **Recovery:** Syntax error, denied write, timeout, stale request, invalid saved query and canceled
  confirmation retain entered SQL and useful result/error context.
- **Screens/modes:** Sql QVue, SqlTui, SqlDownload; browser keyboard, actual terminal code editor and downloaded
  XLSX.

### TOOL-11 — Compare the entity model with the database

- **Actor/start:** DATABASE_ADMIN holder and ordinary reader; deliberate harmless schema difference on scratch
  database.
- **Objective/steps:** Schema/Entities → Entity detail → compare table/model → Check Tables → IndexesKeys plan →
  inspect/download DDL → follow planned job → revisit differences.
- **Expected:** Model versus actual table is clear; planned changes are not marked applied;
  supported/no-op/unavailable capability differences are explicit.
- **Recovery:** Unknown entity, incompatible DDL, missing backend feature, canceled apply and one independent
  statement failure keep other results and name the failing statement.
- **Screens/modes:** Schema/Entities/Entity/IndexesKeys, Database jobs and Dev AIDE entity handoff; actual DDL
  file and TUI/Standard.

### TOOL-12 — Investigate database pressure without losing source context

- **Actor/start:** Reader and DATABASE_ADMIN holder; local sessions with a harmless blocker and query
  statistics.
- **Objective/steps:** Database/Overview → Activity → inspect blocker/session → Queries → Tables → Settings →
  select permitted cancel/terminate on review-owned work → return to metrics.
- **Expected:** Database identity and capabilities stay visible; session/query/table links are meaningful;
  unavailable counters are distinct from zero.
- **Recovery:** Finished session, denied cancellation, unsupported statistics, filtered-empty table and failed
  metric leave remaining information usable.
- **Screens/modes:** Overview, Activity, Queries, Tables, Settings and Monitor embeds; no unrelated process
  termination, TUI/Standard tables.

### TOOL-13 — Follow maintenance and connection operations

- **Actor/start:** DATABASE_ADMIN holder plus DATABASE_EXTERNAL holder on dev; local disposable connection and
  maintenance plan.
- **Objective/steps:** Connections → configure/test/select → Tables plan maintenance → Jobs → JobDetail
  follow/run/cancel → inspect outcome; use SpeedTest with a small bounded fixture and reset only fixture
  statistics.
- **Expected:** Connection test, selected database and job state agree; confirmations identify target; proposed
  SQL and executed outcome remain separate.
- **Recovery:** Production external restriction, bad credential, unsupported operation, canceled job and failed
  maintenance leave existing connections/data safe and secret-free.
- **Screens/modes:** Connections, Tables, Jobs, JobDetail/JobDetailTui, SpeedTest, Queries reset; supported
  modes, no performance claim.

### TOOL-14 — Transform a file through an ETL mapping and repair errors

- **Actor/start:** Operator with mixed valid/invalid local file, target entity, value map and dedicated
  scheduled job.
- **Objective/steps:** Etl/Mappings → Mapping choose source/target → match fields → sample/test → correct
  mapping/value map → save → run → Runs/Run errors/download → repair/rerun eligible rows → inspect schedule.
- **Expected:** Sample errors predict row results; mapping version/source/target are clear; successful rows are
  not lost or duplicated; rerun appears only where applicable.
- **Recovery:** Missing field, bad type/value, failed upload, partial run, no failed rows and canceled schedule
  preserve the mapping and explicit per-row outcomes.
- **Screens/modes:** Etl root transitions, Mappings/Mapping/ValueMaps/Runs/Run and Grid Load into…; QVue, actual
  files and supported terminal/Standard fallback.

### TOOL-15 — Discover and invoke a safe service

- **Actor/start:** Operator with local harmless service and valid/invalid inputs; no side-effectful external
  service.
- **Objective/steps:** ServiceReference → ServiceDetail inspect definition/parameters → ServiceRun fill/call →
  inspect output separately from input → follow artifact metrics; RestApi filter/open JSON schema and
  StatusFlows inspect links.
- **Expected:** Source, service identity, inputs and output remain distinguishable; implicit entity service
  selection is truthful; API descriptions and status flows are navigable references.
- **Recovery:** Unknown service, missing required field, service error, permission refusal and output
  serialization problem preserve useful input and explanation.
- **Screens/modes:** Service children, RestApi, StatusFlows, ArtifactStats and Dev AIDE/Monitor handoffs;
  TUI/Standard and actual JSON documents.

### TOOL-16 — Start, stop and compare a short load run

- **Actor/start:** LOAD_RUNNER holder and missing-permission counterpart; safe local ServiceCall script with
  bounded users/duration.
- **Objective/steps:** Load/Scripts → NewRun choose parameters → start → Run observe progress/charts → adjust
  users where offered → stop → Runs → compare with second fixture run → download/import result.
- **Expected:** Target/configuration, live versus final values and stop state are clear; comparison uses the
  selected runs and retains raw result identity.
- **Recovery:** Invalid plan, denied run, failed worker, interrupted page, stop-now/cancel confirmation and
  malformed import retain final/partial outcomes without orphaned work.
- **Screens/modes:** Load root, Scripts/NewRun/Run/Runs/Compare and QVue/TUI twins; no stress benchmark or
  remote target.

### TOOL-17 — Reconcile helper access and cross-app continuation

- **Actor/start:** Operator, restricted reader and outsider with earlier valid fixture parameters.
- **Objective/steps:** Check remaining routes/helpers/exports; switch selected database between safe contexts;
  follow entity/service → Dev AIDE and metric → Monitor → Back; test selected direct denied actions.
- **Expected:** Database/record context remains explicit; helper URLs and file downloads obey the same
  authority; all inventory entries have an outcome.
- **Recovery:** Revoked grant, unsupported backend, missing artifact and blocked output remain named gaps rather
  than successful render coverage.
- **Screens/modes:** All Tools inventory, authored twins and outputs; no duplicate regression/screen walk or
  addon data.

## 5. Existing checks and future verification

This is an available-test map, not a results ledger. Read assertions and suite setup before selecting a run; static
shape checks, offline component fixtures, live browser walks and service rules provide different evidence.

| Component / suite | Existing specs | Coverage and limits |
| --- | --- | --- |
| tools / server | [DataGrid](../runtime/components/base/tools/test/DataGrid.spec.js), [DataViews](../runtime/components/base/tools/test/DataViews.spec.js), [CalcGrid](../runtime/components/base/tools/test/CalcGrid.spec.js) | Grid download ownership/recent entities/terminal rows, saved view fields and Calc rules; not all browser operations. |
| tools / server | [EntityExport](../runtime/components/base/tools/test/EntityExport.spec.js), [DocumentExport](../runtime/components/base/tools/test/DocumentExport.spec.js), [LoadWrap](../runtime/components/base/tools/test/LoadWrap.spec.js) | Export and load-wrapper contracts; no full UI import/snapshot lifecycle. |
| tools / server | [Etl](../runtime/components/base/tools/test/Etl.spec.js), [SqlTui](../runtime/components/base/tools/test/SqlTui.spec.js), [LoadRunner](../runtime/components/base/tools/test/LoadRunner.spec.js) | Mapping/sample/rerun and database settings, terminal SQL editor and load behavior; service/render evidence. |
| tools / server | [AdminOverview](../runtime/components/base/tools/test/AdminOverview.spec.js), [HomeCatalog](../runtime/components/base/tools/test/HomeCatalog.spec.js) | Tools entry capabilities, service XML/output/selection and home navigation. |
| tools / acceptance | [EtlLoad](../runtime/components/base/tools/test/acceptance/EtlLoad.spec.js) | Existing ETL/data-load scenario; reuse fixtures and assertions. |
| tools / acceptance client | [DataGrid](../runtime/components/base/tools/test/acceptance/client/DataGrid.spec.js), [CalcGrid](../runtime/components/base/tools/test/acceptance/client/CalcGrid.spec.js), [LoadRunner](../runtime/components/base/tools/test/acceptance/client/LoadRunner.spec.js) | Live grid edit/paste/undo/save/export, Calc UI and short load start/stop/compare; additional variants stay pending. |

Future acceptance candidates: **ToolsDataWalk** (TOOL-01–09), **ToolsSqlMaintenanceWalk** (TOOL-10–13),
**ToolsServiceWalk** (TOOL-15) and **ToolsAccessWalk** (TOOL-17). Extend EtlLoad/DataGrid/CalcGrid/LoadRunner for
uncovered TOOL-02/03/07/14/16 branches rather than add duplicate journeys. Schema/backend-dependent checks use
isolated local databases, not broad performance or acceptance-all runs. A single Tools acceptance sweep can cover
remaining route rendering; it does not replace these activities.

Follow the [verification policy](ui-review-shared-gallery.md#verification-policy-for-later-execution). Proposed
walk names above describe future acceptance work, not tests created or authorized to run by this document. Keep
focused bug/rule checks in regression and reuse existing sweeps/walks instead of adding duplicates.

## 6. Coverage, findings and running record

### Activity coverage ledger

Statuses: **pending**, **inspected**, **exercised** (pass/fail/partial named separately), **blocked**, **deferred**
(with reason). Existing tests and the archived review do not advance these statuses. Every branch is pending until
execution records it; “exercised” never implies that all modes and recovery paths passed.

| Activity | Status | Outcome / remaining branches | Evidence / findings |
| --- | --- | --- | --- |
| TOOL-01 — Choose the database and resume an artifact | exercised — partial | Home chooser opened the selected entity; returning Home listed that entity and four SQL history entries. Database switching, reopening from recents, denied/missing artifacts and other modes pending. | B01: 01, 10 |
| TOOL-02 — Edit and save a bounded grid | exercised — partial | Selected entity loaded 11 rows; EX_100 Record and Back to Find retained identity. B02 verified both date displays, unchanged editor blur, named filters, exact match, clear and reload. Sorting, saved edits, conflicts and alternate modes remain pending. Inspected the three-row generated entity in Grid; correlated the updated seed with its source. | B01: 02–03; TOOL-UI-001/002; TOOL-B03 |
| TOOL-03 — Open a file or result as a grid and spreadsheet | exercised — partial | Opened entity rows in Calc, changed the sheet, saved, reopened, exported and deleted the fixture. File-source and failure branches remain open. | TOOL-B03 |
| TOOL-04 — Browse and edit related records | exercised — partial | Inspected EX_100 Record values and related-record links. B02 reproduced the filtered Grid → Record → Back to Find path. Browse/master/detail navigation, relationship links, edits, cancellation/refusal and other modes pending. | B01: 03 |
| TOOL-05 — Define and reuse a database view | exercised — partial | Created ReviewEnums in Standard and verified review.closeout in the reopened list. DataViews separately saved checked master and related fields. | TOOL-B03 |
| TOOL-06 — Define, index and inspect a data document | exercised — partial | Earlier walk. Opened /qapps/tools/Data/Documents as John Doe. Visible controls included Batch Size, Index Feed Documents. | TOOL-B05 (current). TOOL-B03; entry-TOOL-06.json |
| TOOL-07 — Load data and inspect per-file outcomes | exercised — partial | See TOOL-B08 for selected differences, limited access and worker recovery. Mixed-file partial success, corrected retry, create-only and cancel exercised in TOOL-B07. The operator-facing file failure is open. Invalid pasted XML and corrected retry verified in TOOL-B06. Earlier walk. Opened /qapps/tools/Data/Load as John Doe. Visible controls included Import Data - Create Only, Import Data - Create or Update. | TOOL-B06, TOOL-B05, TOOL-B03; entry-TOOL-07.json |
| TOOL-08 — Export exactly the intended records | exercised — partial | Verified the bounded EnumerationType export bytes in the snapshot ZIP. The separate EntityExport checks exercised a JSON filter and a restricted entity. | TOOL-B03 |
| TOOL-09 — Retain and reload a disposable snapshot | exercised — partial | Exported the selected EnumerationType records to a ZIP, downloaded its single 19,207-byte XML file and submitted it for import. Job Runs showed the outcome. | TOOL-B03 |
| TOOL-10 — Run, explain and recover a SQL query | exercised — partial | The original browser run exercised Explain, rejected SQL and History. A live TUI submitted SELECT 17 AS review_count and displayed 17. Scoped comparative and interrupted-query branches remain open. | B01: 04–09; TOOL-B03 |
| TOOL-11 — Compare the entity model with the database | exercised — partial | Earlier walk. Opened /qapps/tools/Schema as John Doe. Visible controls included Choose database, Check tables. | TOOL-B05 (current). TOOL-B03; entry-TOOL-11.json |
| TOOL-12 — Investigate database pressure without losing source context | exercised — partial | Earlier walk. Opened /qapps/tools/Database/Queries as John Doe. Visible controls included Database, Choose database. | TOOL-B05 (current). TOOL-B03; entry-TOOL-12.json |
| TOOL-13 — Follow maintenance and connection operations | exercised — partial | Earlier walk. Opened /qapps/tools/Database/Connections as John Doe. Visible controls included open_in_new New connection, Collapse External databases. | TOOL-B05 (current). TOOL-B03; entry-TOOL-13.json |
| TOOL-14 — Transform a file through an ETL mapping and repair errors | exercised — partial | Earlier walk. Opened /qapps/tools/Etl/Mappings as John Doe. Visible controls included Find, Find. | TOOL-B05 (current). TOOL-B03; entry-TOOL-14.json |
| TOOL-15 — Discover and invoke a safe service | exercised — partial | Earlier walk. Opened /qapps/tools/Service as John Doe. Visible controls included Next page, Last page. | TOOL-B05 (current). TOOL-B03; entry-TOOL-15.json |
| TOOL-16 — Start, stop and compare a short load run | exercised — partial | A holder ran ServiceCall, followed it, stopped it and compared it with a second run. Scoped refusal and remaining failure paths are open. | TOOL-B03 |
| TOOL-17 — Reconcile helper access and cross-app continuation | exercised — partial | Consumed Dev AIDE edits through Grid and independently checked Entity Sync through filtered Grid row reads. Remaining steps and recovery branches are open. | TOOL-B03 |

### Findings register

Use `TOOL-UI-001` onward. P1 blocks work or essential understanding; P2 causes substantial friction or misleading
presentation; P3 is a localized refinement. Label evidence as observed behavior, static suspicion or design judgment.

| ID / severity | Actor, activity step, route, mode / viewport | Reproduction and impact | Evidence / owning layer | State / correction / retest |
| --- | --- | --- | --- | --- |
| TOOL-UI-001 / P2 | `john.doe`, TOOL-02 read/TOOL-04 comparison; `/qapps/tools/Data/Grid?entityName=moqui.example.Example` → Record EX_100; Material light, 1920×1080 | **Observed:** Grid displays Example Date `1792400400000` and Example End Date `1792405800000`; the same record's form displays `2026-10-19 04:00` and `2026-10-19 05:30`. Epoch values make dates difficult to scan or compare in the primary record list. | B01 `02-grid.jpg`, `02-grid.txt`, `03-record.txt`; Tools Grid formatter at `Grid.qvue:476–490` and shared `MoquiL10n.js:399–411`. Formatter catches errors and falls back to raw text; the exact grid-response type boundary remains to diagnose. | Fixed and retested. Both date-time columns returned decimal strings in the row JSON and Tabulator cells. Grid converts them only for display. The editor now compares the native input’s normalized value on blur. In Chrome, EX_100 matched Record at 04:00 and 05:30 on 2026-10-19. Opening and leaving Example Date kept the original string and zero undo entries. GridDates also checked null and numeric-looking text.  B02 verified again with the current build. |
| TOOL-UI-002 / P2 | `john.doe`, TOOL-02 column filters; same Grid route; Material light, 1920×1080 accessibility-tree inspection | **Observed:** Ten column filter inputs expose only unnamed “search text field” nodes, although adjacent headers name their columns. Users navigating inputs by accessible name cannot distinguish which field a filter targets. This finding concerns exposed names; actual screen-reader and keyboard task completion remain pending. | B01 `02-grid.txt` (filter nodes 324, 329, 334, 339, 344, 349, 354, 359, 364, 369); Tools `Grid.qvue:418–445` generic Tabulator header-filter configuration. | Fixed and retested. Eligible Tabulator header inputs now receive a name from their column definition. Chrome exposed ten distinct names. Filtering by the named Example ID searchbox with =EX_100 narrowed the list to one row. Clearing restored the rows, and the Example ID and Example Type Enum ID searchboxes remained addressable by name after reload rebuilt the columns.  B02 verified again with the current build. |
| TOOL-UI-003 / P2 | ADMIN, TOOL-02/04, Material Grid → Record | Filter Example ID to =EX_100. Open its Record cell and choose Back to Find. The same entity opens with all 11 rows and an empty filter, forcing the operator to rebuild the selection. Restore the originating filter when returning. | Observed, B02 filtered.jpg and record.jpg. Grid/Record return navigation. | Fixed and checked 2026-11-25. The Record cell sends the Grid's header filters as a `gridFilters` JSON parameter. Record keeps it through Update and the levels form, and Back to Find hands it back. Grid restores it through `initialHeaderFilter`, as a saved view does, keeping only filters of columns it has. Record's unwritten `toolsEntityFind:` session read is gone. In the built-in browser, typing =EX_100, opening Record, updating Description and choosing Back to Find gave one EX_100 row and the =EX_100 filter. DataGrid and GridDates specs cover it. |
| TOOL-UI-004 / P3 | ADMIN, TOOL-02, Material Grid | Apply the exact-match filter above. The count reads “1 rows”. Use a singular label for a single result. | Observed, B02 filtered.jpg. Grid row count. | Fixed and checked 2026-11-25. Grid's `rowsText` chooses “row” or “rows” through `moqui.l10n.localize` for the paged count, Count rows, an export's note and the file dialog. Live, the filtered count and Count rows read “1 row” and the unfiltered list “11 rows”. GridDates covers it. |
| TOOL-UI-005 / P2 | John Doe, TOOL-07, Standard Data Load | Submit XML containing a valid Enumeration and a second row with an unknown enum type. The response is an HTTP 500 page with “commit failed”. Neither row is stored and the page identifies neither the failing row nor the field. Correcting the reference permits both rows to load. | TOOL-B05, `controls.json`. Tools Load and framework commit-error presentation. | **Fixed 2026-11-26.** The `load` transition runs in the screen's transaction, which holds writes in the transaction cache, and the loader joined it. The text's rows were sent only at the screen's commit, where the database refused the unknown type. That surfaced as "commit failed", which named no record. The loader binding now sets the caller's transaction aside, as Java's `internalRun` does, and each text and file loads in a transaction of its own. A text's writes each wait for their result. A write that fails names its record (`Could not store moqui.basic.Enumeration [enumId=…]`) and carries the database's reason and DETAIL. The Load form catches a failed text and shows "Nothing was loaded from [XML Text Box]: …" as a form error, with the submitted parameters kept (`error-response save-parameters`). Tests: moqui-core `a_load_in_a_cached_transaction_fails_at_its_record_and_the_caller_commits` fails on the old code. The tools spec DataLoadStore posts the two-row XML and checks for the error naming record SPEC_DLS_E2 and the unknown type, with neither row stored. It then posts the corrected text and checks that both rows are stored. Verified live in TOOL-B06. |
| TOOL-UI-006 / P2 | John Doe, TOOL-07, Standard Data Load | A component batch with one rejected file reported only the successful row count. | Data Load component-file result. See TOOL-B07. | Verified live on c89d193f6 in TOOL-B09. |
| TOOL-UI-007 / P2 | John Doe, TOOL-07, Standard Data Load with the foreign-key worker selected | Entering text in an integer field produced a raw HTTP 500 during job preparation. | Data Load worker preflight, TOOL-B08. | Verified live on c89d193f6 in TOOL-B09. |

### Bounded batch record

For each later batch record:

- Intent, activities and branch limits; runtime/build/components, identities/grants, fixture IDs and dates.
- Observed steps and outcomes; evidence filenames, console/network/output observations and findings.
- Workarounds and blocked/deferred branches, with prerequisites and follow-up.
- In refinement only: decisions, owning files/components, correction, focused verification commands/results and retest.
- Temporary settings/grants/files/jobs restored, remaining cleanup and companion handoffs.

### B01 — Read records and recover a SQL query (2026-11-16)

**Intent and boundary.** Start TOOL-01/02/04/10 with existing demo data and read-only SQL. The parent reviewer
performed the visible browser walk; a supporting agent inspected source and reconciled this record with the saved
accessibility snapshots. TOOL-11 schema navigation was considered but not exercised; it remains pending. No data
editing, database switching, schema/maintenance/load action, provider connection or application change was attempted.

**Runtime and actor.** Checkout `83cf05b3b6b069812a6ac2802d95f918db6a9802`; rebuilt debug binary `moqui 0.1.0`,
SHA-256 `f799a82c681a7c3c9a7973cf5e030a5d1ebf41fa8935280f0d527dbf61bb2004`. Isolated runtime
`/tmp/mr-ui-four-apps-zf5098dh`, `http://localhost:18872`, fresh embedded PostgreSQL; framework, webroot, tools,
basic-comms, dev-aide and example components, no addons. Scheduled jobs disabled; telemetry sampling was an explicit
startup fixture. Actor `john.doe` / `EX_JOHN_DOE` with `ADMIN` and `ADMIN_ADV`; this broad demo identity does not
establish the planned restricted-operator or refusal outcomes. Material light, default comfortable density,
1920×1080 only. Review records:
`moqui.example.Example`, existing demo rows EX_100–EX_110; no review-owned business rows were created.

**Observed sequence and outcomes.** Evidence below is under ignored
`plans/ui-refinement-evidence/tools/batch-01/`; it supports only the named branches.

| Step | Visible actions and result | Evidence |
| --- | --- | --- |
| Home → Grid | Used Open an entity to select `moqui.example.Example`; selection opened Grid automatically. Grid showed the entity identity and 11 rows. | `01-home.txt`, `02-grid.txt`, `02-grid.jpg` |
| Grid → Record → Grid | Opened EX_100's Record cell; form showed `Warehouse layout study`, status `EXST_IN_DESIGN`, relationship links and readable dates. Back to Find returned the same entity and 11 rows. Raw Grid dates and unnamed column filters are TOOL-UI-001/002. Related links and Update were not activated. | `02-grid.txt`, `03-record.txt`; return observed in live walk |
| SQL read and Explain | Entered SQL from Tools navigation; ran `select 1 as review_value` with changes off. Result showed one row/value `1`. Explain the statement opened a plan dialog with Result, estimated rows `1` and cost `0.01`. | `04-sql-result.txt`, `05-sql-explain.txt` |
| Invalid SQL and recovery | Ran `select from`; inline PostgreSQL syntax error appeared, editor retained the statement and prior results were removed. Corrected to `select 2 as review_value`; result showed one row/value `2`. | `06-sql-invalid.txt`, `07-sql-recovered.txt` |
| History and Home | History listed all four runs, including Explain and the failed statement. Selecting the EXPLAIN entry populated the editor and closed History; existing value-2 result remained, with no automatic execution. Home's Pick Up Where You Left Off listed the entity and four queries. | `08-sql-history.txt`, `09-history-selected.txt`, `10-home-return.txt`, `10-home-return.jpg` |

**Supporting inspection.** `source-preflight.md` records navigation/service anchors, the Grid formatter fallback
and filter/action semantics. Source inspection is not extra live coverage. Record's JSON display also represents
timestamps as strings; the Grid's actual response type was not captured, so this does not establish the formatter's
root cause. Record cell keyboard activation remains pending; source-only click-handler inspection is not a keyboard
failure finding. No tests were executed and no application files changed.

**Remaining branches and cleanup.** All unmentioned activities and branches remain pending, including schema,
mutations/conflicts, access restrictions, exports, dark/compact/mobile layouts, a full keyboard walk, TUI and Standard.
The four SQL runs created history/audit state in the scratch database; SQL itself did not mutate queried data and
Allow changes stayed off. Runtime lifecycle and disposal are coordinated by the parent review batch; no shared
database, persistent grant or global setting was changed by this Tools walk. `runtime-metadata.json` and
`UiReviewConf.xml` retain the exact environment setup. Preserve findings for consolidated triage before refinement.

**Coordinated shutdown.** After all four browser batches, root stopped the server with SIGINT at
2026-11-17 00:02:38 UTC (Nov 16 locally), exit 0. Server/HTTP listener and runtime PostgreSQL processes/PID file
were absent. Viewport override reset and root tab closed; Logout confirmation was not submitted, so no logout
success is claimed. Curl cookie file removed; scratch runtime/database and ignored local evidence retained.
See `runtime-cleanup.json` in the batch evidence directory.

### B02 — 2026-11-25, Example Grid and Record recheck

Checkout 6d8bb49f8a7c, isolated dev runtime on port 18765, Chicago, demo ADMIN john.doe, Material light at measured 1280×720. Opened moqui.example.Example with 11 demo rows. EX_100 displayed 2026-10-19 04:00 and 05:30 in both Grid and Record. Double-clicking Example Date and leaving it untouched restored its formatted value with Save disabled. All ten searchboxes exposed distinct column names after reload. Exact ID filters for EX_100 and EX_101 narrowed the list; clearing restored all 11 rows. Back to Find restored the entity but cleared the active filter. Evidence is under `plans/ui-refinement-evidence/tools-20261125/`. No record edits, tests or application changes.

### TOOL-B03 — November 25 continuation

John Doe opened the generated RoundReviewAppItem in Material Grid. The two demo IDs and DEVA_SEED_01 were visible. After the Dev AIDE guided save and load, the seed row alone showed “Seed via original guided tab.” The live TUI opened the same entity. Filtered Grid reads on two independent databases verified the Entity Sync receiver was empty after the redirect and held one RR_SYNC_SAMPLE after the corrected retry. See `tool-grid.json`, `seed-final-grid.json`, `tui.txt` and `sync-corrected-receipt.json`.

In a 120×40 TUI the SQL editor returned review_count 17, and the filtered schema opened Grid. The existing browser walks edited and pasted a range, undid, saved and exported it, carried an entity into Calc, and started, stopped and compared two load runs. See tui-writes.txt and walk-checks.txt. ETL service checks are in supporting-checks.txt.

Standard created ReviewEnums and reopened its row. A separate EnumerationType snapshot downloaded as a ZIP with one 19,207-byte XML member. The import returned job 100058 and Job Runs listed the snapshot jobs. See view-readback.json, closeout-enumtypes.zip, snapshot-import.json and snapshot-job-runs.json. SqlServices initially lacked pg_stat_statements on the minimal conf. All four cases passed with the development datasource configuration (sql-final.txt).

Local evidence is under `plans/ui-refinement-evidence/closeout-d10e22f/`.

#### Remaining branches

The ledger records the strongest observed outcome. The remaining steps, recovery paths and modes in each section 4 activity are deferred to the fixture and next check below. An explicitly blocked or failed result keeps that status.

| Activity | Required setup and next observation |
|---|---|
| TOOL-01 | Switch between two available databases and verify a resumed artifact retains its source. |
| TOOL-02 | Compare the saved grid change as a scoped reader and recover from a conflicting second writer. |
| TOOL-03 | Use an actual uploaded file and a failed source read to test the remaining grid-to-sheet paths. |
| TOOL-04 | Edit a related record and verify the original filtered selection survives a validation error. |
| TOOL-05 | Open the saved view through Grid and export exactly the selected master and related fields. |
| TOOL-06 | Index a bounded document definition into a local index and compare its stored fields. |
| TOOL-07 | Load mixed-validity files and verify each failure, successful row and safe retry in Grid. |
| TOOL-08 | Compare a scoped filtered database export with the selected fixture rows and denied fields. |
| TOOL-09 | Read the reloaded fixture in Grid and repeat import with a deliberately invalid snapshot. |
| TOOL-10 | Compare Explain with a live query under separate SQL and read-only grants. |
| TOOL-11 | Use a disposable schema mismatch and compare the model with the physical table after correction. |
| TOOL-12 | Create a bounded slow query and follow its identity from statements to the underlying activity. |
| TOOL-13 | Use a registered external database to test connect, failure and recovery in the connection UI. |
| TOOL-14 | Use a bounded input file and mapping, correct one bad source row and verify the output bytes. |
| TOOL-15 | Invoke a harmless discoverable service with valid and invalid inputs and inspect its outputs under both grants. |
| TOOL-16 | Repeat the same small load run without its grant and exercise interrupted-run recovery. |
| TOOL-17 | Repeat the same entity handoff in the alternate mode with a scoped consumer. |

### TOOL-B04 · November 26 change-led review

The standalone Maps branch is now owned by MAP-01–08. Tools carries its Grid and shared connection-form handoffs. Under TOOL-17 the operator used Grid on two independent databases to verify that the first sync window delivered zero rows and the next delivered the selected row once. Maps Configuration separately paired a grant holder with a Maps-only user. The compared revisions added no standalone Tools XML route outside Maps. See sync-consecutive.json and the [Maps activity register](ui-review-maps.md).

## 7. Decisions

- IDs are `TOOL`; every current Tools definition and supporting custom editor is covered below.
- Calc source acquisition belongs here; Dashboards owns shared/profile-based reporting. System owns registered
  transfer/backup management, so neither plan repeats the other's complete workflow.
- Load testing is reviewed through short bounded local runs. Production maintenance and comparative benchmarking
  are outside scope.

- Preparing or auditing these plans changes documentation only. Application/API changes, test runs, runtime walks
  and fixture creation belong to separate review/refinement execution.
- Current code, mounts and grants take precedence over older plans. Historical outcomes, permissions and exceptions
  are not inherited. Preserve routes, stored preferences and public contracts in any later refinement.
- Finish a review with explicit branch outcomes and a candid unresolved ledger; missing prerequisites do not become
  passes. Building absent features is outside this review.

### TOOL-B05 · November 26 inspection follow-up

Build `fa75eceb6`. These entries update the activity ledger above. The earlier batches retain the original observations. Local receipts are under `plans/ui-refinement-evidence/inspection-fa75/`.

| Activity | Current evidence | Remaining observation | Receipt |
|---|---|---|---|
| TOOL-06 | The real local data-feed fixture indexed the bounded TestEnum definition, checked the document type and index, read back TcRed and exercised deletion. The separate mixed-index fixture checked typed documents and allowed row links. | Submit Index Feed Documents through Tools, follow that job ID and compare the indexed values from its screen. | `index.txt`, `consumers.txt` |
| TOOL-07 | The Standard Load form submitted two XML rows, one with a missing enum type. HTTP 500 reported “commit failed”. Grid showed neither row. Correcting the bad type and retrying stored both rows. | Rechecked in TOOL-B06. Subsequent create-only, cancel and independent-file evidence is in TOOL-B07. | `controls.json` |
| TOOL-11 | Tools SQL renamed the Example status index in the disposable database. The catalog confirmed the original name was missing. Schema’s Check Table ran one CREATE INDEX with zero failures. A second catalog read found idxexamplestatusitem again. | Repeat as a Schema reader without DATABASE_ADMIN on the same missing-index fixture. | `finalwalk.json` |
| TOOL-12 | A bounded slow SQL call returned 75 after 5,005 ms. Queries explained that pg_stat_statements was disabled. On the statistics-enabled runtime Activity showed the PgSleep wait and the second marked query finished in 9,007 ms. | Correlate a single visible PID and normalized statement ID across Activity and Queries. The independent lookups returned no matching row for the marker. | `followup.json`, `verified-controls.json` |
| TOOL-13 | On the disposable runtime configured for private-network database connections, Test identified PostgreSQL 18.6 on 19876, failed on unused port 19875, and identified PostgreSQL 18.6 after the port and password were restored. The registration was then deleted. | Repeat on the read-only grant and compare the connection audit entries. Changing a destination requires credential re-entry. | `verified-controls.json` |
| TOOL-14 | A two-row CSV uploaded through ETL. Sample identified the invalid integer. Run 100002 returned a 171-byte error CSV containing ETL_FA75_TWO. After correcting and uploading the source, run 100003 left rows ONE=12 and TWO=13 in Grid. Run Detail reported the conversion failure on the first run and “No errors” on the corrected one. A separate 100,000-row acceptance load passed. | Rerun only the eligible failures through Run and compare error counts and duplicate handling under the selected load mode. | `additional.json`, `recovery.json`, `etl.txt` |
| TOOL-15 | Service Run echoed ui-inspection-fa75, ping and zero through its valid path. Standard displayed the bad-number conversion error. The same valid echo through runJson was refused to sec.view.only on the all-action transition. | Run the same grant comparison through the alternative terminal or output entry. | `controls.json`, `followup.json`, `services.txt` |

### TOOL-B06 · November 26 live fix verification

John Doe submitted the original two-row XML in Standard Data Load. The form returned HTTP 200 and retained the full XML. The error named Enumeration UI_FA75_LOADB, the enum_type_id field and its missing value NO_SUCH_TYPE. An independent Grid read returned neither row. After changing only the missing type to ExampleType and resubmitting, Grid returned both IDs with that type. Tested in Chrome at 1920×1080 on `8eaf230b3`. [Local browser receipt](ui-refinement-evidence/recheck-8eaf/results.json).

### TOOL-B07 · Independent files, create-only and cancellation

On `8eaf230b3`, John Doe submitted two component XML files together in Standard Data Load. Grid showed NEXT_GOOD stored and NEXT_BAD absent. Correcting the second file and retrying produced both rows. Canceling create-only sent no POST and left both tested values unchanged. Accepting create-only preserved the existing description and inserted NEXT_NEW. Create-or-update then changed the existing description.

**TOOL-UI-006 · P2 · Open.** For the mixed result the page reported “Loaded 1 records” and showed no file error. The server logged the rejected NextB.xml foreign key. An operator using the page cannot identify which input to repair. Recheck with one good file and one bad file and verify that the form names both outcomes. This is the component-file path. TOOL-B06 covers pasted XML. [Control receipt](ui-refinement-evidence/next-8eaf/load.json) and [mixed-file and retry receipt](ui-refinement-evidence/next-8eaf/files.json).

### TOOL-B08 · Selected differences, worker results and recovery

John Doe used the fresh review database and a review-only child table with one foreign key. Check Data displayed the existing field difference, a missing row with a complete key, and a row with an incomplete key. Grid confirmed checking left the existing value at 1 and the proposed new row absent. In Material, John selected the first two rows and applied them through Row Actions. Grid then returned 7 for NEXT_WRAP and 8 for NEXT_CHOSEN. Standard CSV, JSON and a named local XML resource each produced the expected stored value. The scoped user could open Data Load, had no foreign-key worker control, and was refused the same import action. Grid showed no denied row. [Check and refusal](ui-refinement-evidence/next-8eaf/worker-and-check.json), [selection](ui-refinement-evidence/next-8eaf/check-selection.json), [source formats](ui-refinement-evidence/next-8eaf/load-inputs.json).

| Service job run | Result observed in the job record, Database Job Detail and Grid |
|---|---|
| 100001 | The drop and restore jobs both finished Done, including constraint validation. Grid returned NEXT_WRAP with its original value of 1. |
| 100004 | An overlength key failed the load. The service run recorded the failed write and successful restoration. |
| 100005 | Loading a child with a missing parent succeeded while the key was down. The separate validation job failed. John corrected the reference, retried job 100005, and both restore statements reached Done. |

**TOOL-UI-006** also affected an unreadable named resource. Standard reported “Loaded 0 records” when the server logged that the file did not exist. **TOOL-UI-007** occurred before the worker started. The HTTP 500 named the invalid integer field but replaced the Load form. The later worker results above came from separate validly typed submissions. [Worker and missing-file receipt](ui-refinement-evidence/next-8eaf/worker-and-check.json), [failed jobs and recovery](ui-refinement-evidence/next-8eaf/worker-recovery.json).

### TOOL-B09 · November 26 live Load error rechecks

On `c89d193f6`, John Doe used the Standard Load form on a fresh database. Both findings passed.

| Finding | Observed form result | Independent check |
|---|---|---|
| TOOL-UI-006 | A two-file component load reported one record loaded and identified C89B.xml and its missing foreign-key value NO_C89_TYPE. A missing local resource was named alongside a zero count and the file-not-found reason. | Grid contained C89_FILE_GOOD alone after the partial load. Correcting C89B.xml and retrying produced a two-record result and both IDs in Grid. |
| TOOL-UI-007 | With the foreign-key worker selected, a noninteger value returned HTTP 200 on Load. The form retained the exact XML, identified the field and value, and said no job had started. A missing resource submitted through the worker also remained on the form with its location and reason. | The test record was absent. The DatabaseLoadWrap service-run list and Database Jobs screen were empty before and after both worker submissions. |

[Standard form, Grid and job observations](ui-refinement-evidence/recheck-c89/tools-load.json).

## 2026-11-27 coordinated review at `23204bf61`

The SQL picker listed transactional and content. The grant holder ran `select 17 as review_value` and received 17, while Explain returned a PostgreSQL Result node with output 17. Both requests from the view-tier browser returned 403. The Data Grid browser walk edited cells, pasted a range, performed Undo, saved, downloaded and restored the edited rows. The Load walk exercised start, stop, history and comparison. [SQL and Grid receipts](ui-refinement-evidence/pass-2320/A/tools.json) · [Browser walks](ui-refinement-evidence/pass-2320/A/browser.log)


### Activity dispositions for the coordinated pass

The row records the furthest observation supported on this build. Each remaining check is deferred until its named setup and observation. Dated batches above still document their original actor and mode. [Per-activity accounting and original protocols](ui-refinement-evidence/pass-2320/A/accounting.json)

| Activity | Evidence on the pinned build | Remaining fixture and observation |
|---|---|---|
| TOOL-01 | Current partial. SQL listed transactional/content, switched between them, ran the known query under each selection and restored the initial database. [Receipt](ui-refinement-evidence/pass-2320/A/tools-recheck.json) | Open a previously saved or recent artifact after changing databases and compare the persisted source with the resumed consumer. Both groups currently resolve to embedded PostgreSQL. |
| TOOL-02 | Current partial. A read-only user saw the first writer's changed row; a second writer's stale stamp was refused. Reloading and saving persisted the reconciled text. A scoped write was refused. The browser walk also pasted, undid, saved and exported. [Receipt](ui-refinement-evidence/pass-2320/A/tools-recheck.json) | Save/reload the personalized view and verify multi-page lookup, delayed response and canceled source change against the restored selection. |
| TOOL-03 | Current partial. Uploaded a real CSV through the Grid file provider. Grid returned Alpha/17 and Beta/43, Calc opened the sheet and a missing file returned a named error. The independent workbook file case also passed. [Receipt](ui-refinement-evidence/pass-2320/A/final-groups.json) | Use Grid's picker to reopen an uploaded multi-sheet workbook, exercise malformed workbook/empty sheet/cancel and compare a saved formula and exported file with the original. |
| TOOL-04 | Current partial. Filtered Browse to REVIEW_SNAPSHOT_A, followed its edit link, observed the relationship group, and browser Back returned the same row and filter. Actual TUI Grid Enter opened its Record fields. [Receipt](ui-refinement-evidence/pass-2320/A/workbook-recheck.json) | Follow a related row, submit an invalid field, correct it and verify both the edited relation and original filtered selection. Use separate write/read roles. |
| TOOL-05 | Current partial. Saved two master aliases and a related alias. Grid read SsrLink, A link, SMS Spam Rule Type. The actual one-row CSV contained exactly enumId, description and description2 with those values. [Receipt](ui-refinement-evidence/pass-2320/A/final-groups.json) | Test an invalid alias or relation, restore it and verify the same saved view in alternative modes and under a scoped reader. |
| TOOL-06 | Current partial. BusinessDatasets built a registered feed's DataDocument, distinguished unbuilt data and scope, and refused removal of a depended-on field. DocumentExport distinguished an included render from a write. [Receipt](ui-refinement-evidence/pass-2320/A/continuation.log) | Carry an edit through the Documents UI, rebuild the index, inspect the exact changed document and its authorized export, then correct a bad field. |
| TOOL-07 | Current partial. DataLoadStore exercised selected differences, run authorization and limited fields. Snapshot import independently produced a Grid-readable row. The prior two Tools Load finding closures retain their narrower original receipts. [Receipt](ui-refinement-evidence/pass-2320/A/rules.log) | Use a multi-file submission through Data Load and correlate each completion/retry with its row and original file; the snapshot issue belongs to TOOL-09. |
| TOOL-08 | Current partial. A holder downloaded the selected Enumeration ID and description as CSV. The read-only user saw the source row, but its gridExport POST was refused. Registered per-field export rules passed separately. [Receipt](ui-refinement-evidence/pass-2320/A/tools-recheck.json) | Give two profiles actual export permission with different row/field scopes, activate Export in both and compare downloaded bytes with their allowed data. |
| TOOL-09 | Current failure. Valid ZIP imported one row and Grid read it; two differently named and independently generated corrupt ZIPs finished with Error recorded No and recordsLoaded 0. Server log records the parsing errors. Open finding TOOL-UI-008. [Receipt](ui-refinement-evidence/pass-2320/A/confirmation.json) | Recheck both corrupt fixtures after the loader reports the file-level failure in Job Runs. Compare successful archive and failed artifact in Snapshot history and repeat under restricted roles. |
| TOOL-10 | Current partial. Browser screen transitions returned 17 and a Result plan to the SQL holder. Both were refused with 403 to the view-tier user. Actual terminal SQL editor executed the same query and displayed 17. [Receipt](ui-refinement-evidence/pass-2320/A/terminal.txt) | Exercise cancellation, named inputs, large-page handling and connection/syntax errors in the editor; compare the selected database on return from result Grid. |
| TOOL-11 | Historical partial. The prior inspection and comparisons remain attributable to their original fixtures. Dated activity ledger | A review-owned physical table deliberately differing from its registered entity. Compare both definitions in Schema, correct the fixture and verify the displayed discrepancy disappears. |
| TOOL-12 | Current partial. The SQL holder ran a tagged, five-second read-only statement. Another browser observed active Timeout: PgSleep and the wait disappeared when the query returned 17. Statements reported that pg_stat_statements was not loaded. [Receipt](ui-refinement-evidence/pass-2320/A/database-activity.json) | Enable pg_stat_statements on a disposable datasource, restart it, and match the query ID between Activity and Statements. The live wait by itself did not identify the tagged row. |
| TOOL-13 | Current partial. A disposable PostgreSQL 18 consumer answered the registered external connection after applying the local private-network fixture setting. The wrong port failed, restoring it recovered the connection, and the connection row and setting were removed. [Receipt](ui-refinement-evidence/pass-2320/A/external-recovery.json) | Exercise an authorized maintenance plan, bounded speed test and job cancel/reopen on a disposable schema. Verify per-connection history and non-admin recovery. |
| TOOL-14 | Current partial. The ETL acceptance walk loaded a real 100,000-row CSV. Etl also exercised file matching and row errors in a bounded mapping. [Receipt](ui-refinement-evidence/pass-2320/A/etl.log) | Correct one bad input row through the mapping UI, repeat only its work and compare actual output bytes/record identities before and after correction. |
| TOOL-15 | Current partial. Separate browser sessions called the harmless noop via Service Run. The view-only user's transition was denied and the holder received a service response. [Receipt](ui-refinement-evidence/pass-2320/A/confirmation.json) | Select the discoverable service from the tool itself, submit a typed invalid input and its corrected counterpart, and compare documented outputs in the browser and TUI. |
| TOOL-16 | Current partial. The same bounded ServiceCall form returned 403 to the reader and completed two requests with zero errors as holder. The shipped browser walk also stopped a live run and compared its persisted result with a second run. [Receipt](ui-refinement-evidence/pass-2320/A/final-groups.json) | Interrupt the worker or server while retaining its results volume, then reopen that run and compare its interrupted state and available output. A normal Stop is already recorded. |
| TOOL-17 | Current partial. The actual TUI selected a Grid row and entered Record with its edit fields. The browser's uploaded data handoff to Calc showed its header and named the missing-file refusal. [Receipt](ui-refinement-evidence/pass-2320/A/terminal.txt) | Use the same scoped identity and selected row through both consumers, then remove its row grant, retry the handoff, and return to the originating selection. |

**TOOL-UI-008 · P2 · Open.** Snapshots accepted a valid ZIP and two corrupt ZIPs. Jobs 100028 and 100033 finished with zero rows, an explicit “No” for Error recorded and empty error details. The loader logged an unreadable archive for both. The control job imported one known row that Grid could read. Framework data-loader error propagation owns the failure. Tools Snapshots, System Job Runs and the jobs readiness check consume that status. Recheck a mixed valid/bad input as well as a wholly unreadable archive after repair. [Completed job receipts](ui-refinement-evidence/pass-2320/A/confirmation.json) · [Server errors](ui-refinement-evidence/pass-2320/A/server-errors.txt)

The isolated runtime and PostgreSQL container stopped. The four seeded report schedules were paused, the original configuration and account state were verified, and no listener remained on the assigned ports. [Cleanup receipt](ui-refinement-evidence/pass-2320/A/cleanup.json)
