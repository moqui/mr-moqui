# MR Moqui — UI review: Dashboards

**Review update — 2026-11-25.** DASH-B02 records the latest live evidence. The activity ledger identifies the branches still open.

**Status — 2026-11-25, refinement.** All four DASH-B01 findings are fixed (DASH-UI-001–004). DASH-UI-003 was a shared `m-form-link` defect that dropped the clicked submit button for every form to a screen.

**Previous status — 2026-11-25.** DASH-B01 opened all 13 activities on the current release. Four findings concern unsupported dataset selection, misleading record times, the Analysis Run action and raw Git sync timestamps. Branch-level outcomes and remaining work are recorded in §6.

Contents: [scope](#1-purpose-scope-and-sources) · [method](#2-review-method-and-refinement-criteria) ·
[inventory](#3-screen-and-surface-inventory) · [activities](#4-end-to-end-activities) ·
[verification](#5-existing-checks-and-future-verification) · [record](#6-coverage-findings-and-running-record) ·
[decisions](#7-decisions).

## 1. Purpose, scope and sources

Review reporting as a reader, analyst and distributor: find a trustworthy result, explain it down to its permitted
records, build a reusable view, and deliver the same scoped information to the right people. Include technical and
framework-only datasets; no Mantle/BOSS dataset or organization model is required.

Sources: [tools README](../runtime/components/base/tools/README.md), [Monitoring/reporting guide](../docs/tools/Monitoring.md),
[CalcGrid guide](../docs/tools/Database.md), [IronCalc plan](IronCalc-DB.md),
[report-screen extraction](archive/Admin-Screens-Down.md), current `tools/screen/Dashboards`, `tools/template/Report*.js`,
`CalcGrid.js`, `tools/report`, `MonitoringSecurityData.xml` and framework report/calc services.
The [old review](archive/UI-Refinement.md) supplies historical authoring/export/playlist observations, not current results.

### Ownership and handoffs

| Surface | Owner / boundary |
| --- | --- |
| All Dashboards routes, definition/panel editors, dataset stewardship, CalcGrid, delivery/alert/playlist UI | This plan |
| Monitoring's embedded panels and telemetry-specific drill | [Monitor](ui-review-system-monitoring.md); this plan owns report engine controls/definition authoring |
| Tools Grid's Calc source acquisition | [Tools](ui-review-tools.md); this plan owns shared-document, profile and report consumption |
| Scheduled job/mail readiness and recovery | [System](ui-review-system.md) and [AI/integrations](ui-review-system-ai-integrations.md); follow delivery evidence from here |
| Reused screens mounted by other applications | Inspect framework `ReportMounts` fixtures as supporting evidence; addon Insights is outside the review runtime |

System and Dashboards roots null report-mount context. Preserve that boundary and parameter context in embedded
links; do not treat an app-supplied policy map as authorization.

## 2. Review method and refinement criteria

Apply the [common review protocol](ui-review-shared-gallery.md#common-review-protocol) and
[verification policy](ui-review-shared-gallery.md#verification-policy-for-later-execution). The details below add this
area's fixtures, dependencies and judgment criteria.

### Environment, identities and records

| Proposed actor / fixture | Grants and use |
| --- | --- |
| `review.report.author` / `review.report.reader` | Prepared `DASHBOARDS_APP` grants and dataset/definition ownership or sharing sufficient for the selected activity; reader cannot acquire author rights from a button |
| `review.report.recipient` / `review.report.outsider` | Recipient with equivalent dataset scope versus a different/no scope; use framework-only fixture data modeled on ReportAudience/ReportDrill |
| `review.calc.author` / `review.calc.reader` | `CALC_GRID`, author additionally `CALC_GRID_AUTHOR`, explicit `org.moqui.calc.profile.<id>` and admitted entity grants/filter sets; no Tools grant to bypass the profile |
| `review.report.distributor` | Explicit group-delivery/share authority as required by `REPORT_DELIVERY_ADMIN` / `REPORT_SHARE_ADMIN`; do not assume DASHBOARDS_APP alone grants them |
| `review.admin` | Setup, unrestricted dataset administration, local Git/contact configuration; not the restricted-reader counterpart |

Prepare a copied shipped dashboard, owned library panel, registered framework DataDocument/data feed with several
groups/units, a multi-page dataset, two shared Calc documents, equivalent and non-equivalent audiences, a failed and an uncertain
delivery, and a playlist with an inaccessible item. Names/IDs belong to this review only. Use local mail/webhook
capture and a disposable Git remote; PDF requires local Chromium. Record scheduler state and restore schedules,
shares, alerts and server resource locations. `REPORT_SAVE_LOCATION` is separate from delivery permission.
Include a two-dashboard report definition and separate recipient-checked and owner-mode schedules. A later
`ownReportFile` download is a fresh render under the recipient's current access; retain captured sent bytes to
assess the original delivery. It is not a retained copy of the attachment.

### Area-specific criteria

- Time range, timezone, variables, units, dataset scope and definition identity remain visible through drill,
  embedding, exports and delivery. Empty/unavailable/restricted data is never a misleading zero.
- Editing a copy is distinct from changing a shipped definition; versions, preview and Save show what will persist.
  Stale requests must not replace a newer panel, dataset or selected workbook.
- Aggregate semantics and visible records agree; no invalid mixed-unit total or page-limited total masquerades as a
  complete result. Restricted source records remain unavailable even when their aggregate is readable.
- Alert/schedule state distinguishes evaluation requested, skipped, queued, delivered, failed and unknown. Sharing
  and unattended playback preserve recipient scope, and exported bytes use that same scope.

### Ordered waves

| Wave | Activities | Continuation |
| --- | --- | --- |
| 1. Read and explain | DASH-01–03 | Saved search/drill feed authored analyses |
| 2. Steward and author | DASH-04–06 | Dataset → analysis/panel → copied dashboard/version |
| 3. Spreadsheet work | DASH-07–08 | Saved Calc document → shared editing → panel/report |
| 4. Deliver and react | DASH-09–11 | Schedule/delivery, alert, playlist/wall |
| 5. Maintain and reconcile | DASH-12–13 | Annotations/Git/version recovery, access/mount/output checks |

Keep an original copy for later comparisons; do not retire a dataset until its use/version scenarios finish.
Schedule/alert tests consume distinct synthetic occurrences so a repeated delivery cannot be confused with a new one.

## 3. Screen and surface inventory

This inventory accounts for **29 XML definitions** assigned to this file; screen containers and helper
definitions are included. Routes below are Material paths unless labelled otherwise; app routes have Standard
`/apps` counterparts. A file's physical presence does not prove access or a supported interaction in every mode.
Each row maps to §4; helper inspection does not replace its parent workflow. Counts describe definitions, not user
activities. Dialogs, templates and non-XML surfaces follow the route tables.

### tools: `Dashboards`

| Route / surface | XML source | Default / variants / boundary | Activities |
| --- | --- | --- | --- |
| `/qapps/dashboards/AlertRule` | [AlertRule.xml](../runtime/components/base/tools/screen/Dashboards/AlertRule.xml) | Authored text: `html`, `html,html-print,text`, `qvt`, `tui`. Hidden/helper entry; enter through its parent action and inspect direct access. Context: `definitionId`, `dashboardId`, `panelId`, `queryIndex`. | DASH-10 |
| `/qapps/dashboards/AlertShared` | [AlertShared.xml](../runtime/components/base/tools/screen/Dashboards/AlertShared.xml) | Authored text: `qvt,html,html-print`, `tui`. Hidden/helper entry; enter through its parent action and inspect direct access. Context: `definitionId`, `sharedType`. | DASH-10 |
| `/qapps/dashboards/Alerts` | [Alerts.xml](../runtime/components/base/tools/screen/Dashboards/Alerts.xml) | Authored text: `html`, `qvt`, `tui`. Context: `view`, `state`. | DASH-10 |
| `/qapps/dashboards/Analysis` | [Analysis.xml](../runtime/components/base/tools/screen/Dashboards/Analysis.xml) | Context: `datasetId`, `aggregate`, `measureField`, `groupBy`, `groupBy2`, `timeGrain`, `filterField`, `filterValue`, `from`, `thru`, `run`. | DASH-05 |
| `/qapps/dashboards/Annotations` | [Annotations.xml](../runtime/components/base/tools/screen/Dashboards/Annotations.xml) | Includes `component://tools/screen/Dashboards/Explore.xml`. | DASH-12 |
| `/qapps/dashboards/CalcGrid` | [CalcGrid.xml](../runtime/components/base/tools/screen/Dashboards/CalcGrid.xml) | Companions: [CalcGrid.qvue](../runtime/components/base/tools/screen/Dashboards/CalcGrid.qvue). Authored text: `qvue`, `tui`. Declared modes: `vue,qvue,tui`. | DASH-07, DASH-08 |
| `/qapps/dashboards/Dashboard` | [Dashboard.xml](../runtime/components/base/tools/screen/Dashboards/Dashboard.xml) | Context: `filtersJson`, `definitionId`, `panelId`, `from`, `thru`. | DASH-01, DASH-03, DASH-06, DASH-13 |
| `/qapps/dashboards/DatasetDetail` | [DatasetDetail.xml](../runtime/components/base/tools/screen/Dashboards/DatasetDetail.xml) | Authored text: `html`, `qvt`, `tui`. Hidden/helper entry; enter through its parent action and inspect direct access. Context: `datasetId`. | DASH-04 |
| `/qapps/dashboards/DatasetFields` | [DatasetFields.xml](../runtime/components/base/tools/screen/Dashboards/DatasetFields.xml) | Hidden/helper entry; enter through its parent action and inspect direct access. Context: `datasetId`. | DASH-04 |
| `/qapps/dashboards/DatasetView` | [DatasetView.xml](../runtime/components/base/tools/screen/Dashboards/DatasetView.xml) | Hidden/helper entry; enter through its parent action and inspect direct access. Context: `datasetId`, `from`, `thru`, `queryString`, `pageIndex`. | DASH-04 |
| `/qapps/dashboards/Datasets` | [Datasets.xml](../runtime/components/base/tools/screen/Dashboards/Datasets.xml) | Authored text: `html`, `qvt`, `tui`. Context: `status`, `text`, `otherDatasetId`. | DASH-02, DASH-04 |
| `/qapps/dashboards/Definition` | [Definition.xml](../runtime/components/base/tools/screen/Dashboards/Definition.xml) | Authored text: `html,html-print,text`, `qvt`, `tui`. Hidden/helper entry; enter through its parent action and inspect direct access. Context: `definitionId`, `definitionType`, `leftVersion`, `rightVersion`. | DASH-05, DASH-06, DASH-12 |
| `/qapps/dashboards/Drill` | [Drill.xml](../runtime/components/base/tools/screen/Dashboards/Drill.xml) | Hidden/helper entry; enter through its parent action and inspect direct access. Context: `definitionId`, `panelId`, `queryReference`, `from`, `thru`, `variablesJson`, `bucketsJson`, `breakdownDimension`, `pageIndex`. | DASH-03 |
| `/qapps/dashboards/Explore` | [Explore.xml](../runtime/components/base/tools/screen/Dashboards/Explore.xml) | Authored text: `html,html-print`, `html,html-print,text`, `qvt`, `text`, `tui`. Context: `filtersJson`, `pillAction`, `pillIndex`, `pillField`, `pillOperation`, `pillValue`, `pillLabel`, `historyQuery`, `from`, `thru`, `queryString`, `pageIndex`, `pageSize`, `order`, `showPatterns`, `datasetId`, `cursor`, `fieldName`, `savedSearchId`, `retrievalMode`. | DASH-02, DASH-05 |
| `/qapps/dashboards/GitSync` | [GitSync.xml](../runtime/components/base/tools/screen/Dashboards/GitSync.xml) | Inspect the route and its visible controls through the activity. | DASH-12 |
| `/qapps/dashboards/Home` | [Home.xml](../runtime/components/base/tools/screen/Dashboards/Home.xml) | Context: `definitionType`, `folder`, `tag`, `starredOnly`. | DASH-01, DASH-06 |
| `/qapps/dashboards/LogContext` | [LogContext.xml](../runtime/components/base/tools/screen/Dashboards/LogContext.xml) | Hidden/helper entry; enter through its parent action and inspect direct access. Context: `datasetId`, `documentId`, `documentType`, `filtersJson`, `from`, `thru`, `queryString`. | DASH-02 |
| `/qapps/dashboards/Panels` | [Panels.xml](../runtime/components/base/tools/screen/Dashboards/Panels.xml) | Includes `component://tools/screen/Dashboards/Home.xml`. Context: `previewPanelId`. | DASH-01, DASH-05, DASH-06 |
| `/qapps/dashboards/Playlist` | [Playlist.xml](../runtime/components/base/tools/screen/Dashboards/Playlist.xml) | Authored text: `html,html-print,text`, `qvt`, `tui`. Context: `definitionId`, `from`, `thru`. | DASH-11 |
| `/qapps/dashboards/Playlists` | [Playlists.xml](../runtime/components/base/tools/screen/Dashboards/Playlists.xml) | Inspect the route and its visible controls through the activity. | DASH-11 |
| `/qapps/dashboards/ReportDelivery` | [ReportDelivery.xml](../runtime/components/base/tools/screen/Dashboards/ReportDelivery.xml) | Authored text: `html`, `qvt`, `tui`. Hidden/helper entry; enter through its parent action and inspect direct access. Context: `occurrenceId`. | DASH-09 |
| `/qapps/dashboards/Reports` | [Reports.xml](../runtime/components/base/tools/screen/Dashboards/Reports.xml) | Authored text: `qvt,html,html-print`, `tui`. | DASH-09 |
| `/qapps/dashboards/ScheduledReportDetail` | [ScheduledReportDetail.xml](../runtime/components/base/tools/screen/Dashboards/ScheduledReportDetail.xml) | Authored text: `html`, `qvt`, `tui`. Hidden/helper entry; enter through its parent action and inspect direct access. Context: `screenScheduledId`. | DASH-09 |
| `/qapps/dashboards/Schedules` | [Schedules.xml](../runtime/components/base/tools/screen/Dashboards/Schedules.xml) | Authored text: `html`, `qvt`, `tui`. Context: `view`, `definitionId`, `state`, `pageIndex`. | DASH-09 |
| `/qapps/dashboards/Split` | [Split.xml](../runtime/components/base/tools/screen/Dashboards/Split.xml) | Hidden/helper entry; enter through its parent action and inspect direct access. Context: `datasetId`, `queryString`, `from`, `thru`, `compareFrom`, `compareThru`, `leftVersion`, `rightVersion`, `serviceName`. | DASH-02 |
| `/qapps/dashboards/WallDisplay` | [WallDisplay.xml](../runtime/components/base/tools/screen/Dashboards/WallDisplay.xml) | Authored text: `html`, `qvt`, `tui`. Hidden/helper entry; enter through its parent action and inspect direct access. Context: `definitionId`. | DASH-11 |

### tools: `roots`

| Route / surface | XML source | Default / variants / boundary | Activities |
| --- | --- | --- | --- |
| `/qapps/dashboards` | [Dashboards.xml](../runtime/components/base/tools/screen/Dashboards.xml) | Default: `Home`. | DASH-01, DASH-13 |

### Framework report email outputs

| Output | XML source | Activity / inspection |
| --- | --- | --- |
| Scheduled report message | [DashboardReportEmail.xml](../runtime/components/base/framework/screen/DashboardReportEmail.xml) | DASH-09; HTML/text, report link, optional CID image and captured attachment |
| Business alert message | [ReportAlertEmail.xml](../runtime/components/base/framework/screen/ReportAlertEmail.xml) | DASH-10; per-recipient state/labels, no-data/error wording and alert link |

[ReportEmailData.xml](../runtime/components/base/framework/data/ReportEmailData.xml) registers these templates.
They have no independent navigation route; read their actual output after the owning delivery activity.

### Embedded surfaces, dialogs, outputs and reconciliation

| Surface | Activity / inspection |
| --- | --- |
| Definition structured/JSON editor, query choices, preview, versions/compare/restore/import/export | DASH-05–06, 12; the editor is within Definition, not separate routes |
| Dashboard time/variables, panel More/inspect/export/filter/share and embedded report component | DASH-01–03, 06, 13; verify ownership from an embed as well as the full route |
| Explore filters/patterns/field values/recent queries, saved-search and Add panel dialogs | DASH-02, 05; distinguish typed query and human summary |
| Dataset fields/conditions, admission/retire/index rebuild and permitted record outputs | DASH-04; current registered datasets rather than older all-dataset assumptions |
| CalcGrid QVue: document/source/profile chooser, sheets/formula bar/formatting, presence, undo/discard, share, panel, Excel export | DASH-07–08; Tools Grid source behavior belongs to TOOL-02/03 |
| Schedule audience preview, delivery recipient results/retry/reconcile and ownReportFile | DASH-09; inspect final captured attachment bytes and per-recipient access |
| Definition's report composer and Reports' owner-mode schedule form | DASH-09; multiple dashboard references/ranges/variables, output layout, email/file destination and distinction from Schedules' recipient-checked delivery |
| Alert basis/audience/contact/silence/mute forms, shared references, copy/pause/test/evaluate | DASH-10; technical contact-point versus business-audience variants |
| Playlists sharing, normal playback and WallDisplay public-style page | DASH-11; inaccessible items keep their original selection identity |
| Annotations, GitSync and local definition folder | DASH-12; writes stay in disposable resource/repository locations |

CalcGrid, dataset stewardship, scoped delivery and shared alert screens are implemented additions beyond parts of the
archived review. BOSS-only business dashboards, attention records and shared-device registration administration are
not invented here; the common Device flow and any framework wall link remain explicit handoffs.

## 4. End-to-end activities

Use visible controls and inspect intermediate states, confirmation and the resulting data/output. Unless narrowed
below, each activity has Material light/dark, actual supported TUI and a brief Standard/output branch. Select narrow
and keyboard checks where layout/interaction differs. Record each unattempted branch in §6; a service call or render
alone does not complete the activity. Names beginning `review.` are fixture proposals, not shipped accounts.

### DASH-01 — Start from a trustworthy dashboard

- **Actor/start:** Report reader/author with shipped, starred, recent and shared dashboards; an empty folder
  counterpart.
- **Objective/steps:** Home → folder/star/recent → Dashboard → change range/variables → inspect panel → open its
  definition → return; star/unstar and reopen in a fresh session.
- **Expected:** Cards, charts and tables identify range/unit/scope; navigation records the intended recent item;
  variables and single-panel selection survive range changes.
- **Recovery:** No accessible definitions, deleted recent item, failed panel and no data show distinct states
  while independent panels remain usable.
- **Screens/modes:** Home, Dashboard, Panels and panel controls; browser/TUI/Standard, chart legend/axis checks
  and narrow header.

### DASH-02 — Explore a dataset and resume a saved search

- **Actor/start:** Reader of a framework dataset with several fields, timestamps and a known trace/context where
  applicable.
- **Objective/steps:** Datasets → Explore → type filter → inspect field values/pattern → add/remove filter →
  save search → change query → reopen saved search → compare in Split.
- **Expected:** Typed values, range and query match visible rows; reset is distinct from saved-search deletion;
  context/trace links preserve the selected row.
- **Recovery:** Invalid query, unknown field, no matches, delayed response and unavailable context preserve
  editable input and discard stale completions.
- **Screens/modes:** Explore, Split, LogContext and Datasets; browser/TUI/Standard, filters and long-query
  keyboard recovery.

### DASH-03 — Explain a panel value down to allowed records

- **Actor/start:** Scoped reader and aggregate-only counterpart; a dataset with known buckets, an Other bucket
  and multiple units.
- **Objective/steps:** Dashboard → choose bucket/value → Drill → select breakdown → inspect source rows → open
  an offered record/context link → return to the original panel.
- **Expected:** Bucket predicate and scope survive every step; Other retains its predicate; totals and units
  remain meaningful; aggregate-only access reveals no source rows.
- **Recovery:** Inaccessible panel, retired dataset, invalid mixed-unit aggregation, empty bucket and changed
  permission offer truthful limits and no data leakage.
- **Screens/modes:** Dashboard, Drill, Explore/context links; actual exports where offered and TUI/Standard
  record selection.

### DASH-04 — Create and steward a usable dataset

- **Actor/start:** Dataset administrator with a disposable framework DataDocument, admitted fields, a record
  link and invalid-field counterpart.
- **Objective/steps:** Datasets → create/select → DatasetDetail → DatasetFields add/edit fields and conditions →
  validate → admit/rebuild → DatasetView → filter/export → inspect Uses; retire only after dependent authoring.
- **Expected:** Definition, fields, filters and presented records agree; admission/index status explains
  readiness; Uses identifies dependent consumers.
- **Recovery:** Unknown relation/field, invalid condition, failed indexing, empty records and denied export keep
  independent operations usable and name failures.
- **Screens/modes:** Datasets, DatasetDetail, DatasetFields, DatasetView and definition link;
  browser/TUI/Standard, exported document content.

### DASH-05 — Build an analysis and a reusable panel

- **Actor/start:** Author with a permitted dataset and typed measures/dimensions from DASH-04.
- **Objective/steps:** Analysis → choose source/aggregate/dimensions → run → inspect rows → save as library
  panel; Explore → Add panel → Definition → preview → reopen from Panels.
- **Expected:** Source and aggregation are understandable; saved panel reproduces the preview under the viewer
  scope and uses human labels without altering typed predicates.
- **Recovery:** Invalid aggregation, missing dataset grant, unsupported query shape, canceled save and stale
  preview preserve input and explain the problem.
- **Screens/modes:** Analysis, Explore, Definition and Panels; structured controls, JSON fallback and
  terminal/Standard limitations.

### DASH-06 — Assemble, share and revise a dashboard

- **Actor/start:** Author with a copy of a shipped dashboard and owned library panel; a shared reader.
- **Objective/steps:** Home → copy/new Definition → add/reorder/configure panels and variables → preview/save →
  Dashboard → share to permitted group → reader opens → compare versions → restore a chosen revision.
- **Expected:** Shipped source remains identifiable; saved revision matches preview; sharing grants only
  intended access; restore produces the selected content while preserving definition identity.
- **Recovery:** Malformed import/JSON, inaccessible referenced panel, stale edit, canceled delete/restore and
  failed save keep the last saved version and local edits distinguishable.
- **Screens/modes:** Definition, Dashboard, Panels, Home; share/version/import/export dialogs, keyboard panel
  ordering where offered and Standard/TUI support.

### DASH-07 — Create a spreadsheet over admitted data

- **Actor/start:** Calc author with a narrow profile and a reader; separate Tools administrator to establish the
  source handoff.
- **Objective/steps:** CalcGrid → select held profile/entity → open data sheet → add sheet → formula referencing
  data → format → undo → save document → close/list/reopen → refresh → export Excel.
- **Expected:** Source data remains distinct from editable workbook cells; formula results and styles persist;
  reopening re-reads only admitted rows; file values match the current document.
- **Recovery:** Unheld profile, unauthorized entity, empty source, invalid formula, stale refresh and canceled
  save retain a clear recoverable document state.
- **Screens/modes:** CalcGrid QVue and supported terminal sheet; source handoff to Tools Grid, actual XLSX bytes
  and browser keyboard navigation.

### DASH-08 — Edit a shared spreadsheet and consume its range

- **Actor/start:** Two authorized editors, one reader and outsider; saved document from DASH-07 and an owned
  dashboard.
- **Objective/steps:** Share document → open in two sessions → edit different cells → observe presence → undo
  own edit → discard pending edits → add a named range as dashboard panel → reopen as reader.
- **Expected:** Both sessions converge; own undo is distinguishable from another editor change; reader cannot
  edit; panel reads the range through its viewer scope.
- **Recovery:** Concurrent edit, disconnected/reopened page, revoked share, missing range and late response do
  not display another document or leak the underlying data.
- **Screens/modes:** CalcGrid sharing/presence/undo, Dashboard panel and report dataset; desktop/narrow samples,
  TUI documented limits.

### DASH-09 — Schedule a report and resolve individual delivery outcomes

- **Composition and owner-mode branch:** Reports → Compose a multi-dashboard report → Definition with type
  `report` → add two dashboard references with distinct variables/ranges → set layout/paper/orientation/appendix →
  save/reopen → preview each offered output. Inspect CSV/ZIP constituents and PDF/PNG/XLSX where supported; record
  unavailable renderers. Separately create a Reports owner-mode schedule with local email/file destinations, then
  open it in ScheduledReportDetail. Confirm its explanatory owner-mode text and retained recipients/location;
  do not expect that editor to convert it to recipient-checked mode. Cancel one creation and reject an ungranted
  save location before completing valid fixture delivery.
- **Actor/start:** Distributor, equivalent-scope recipient and non-equivalent recipient; local mail capture,
  scheduler and PDF/XLSX renderer as required.
- **Objective/steps:** Reports → preview → Schedules create → ScheduledReportDetail edit/audience preview → Send
  now → ReportDelivery → inspect each recipient outcome/file → retry failed recipient or reconcile unknown →
  pause/reopen.
- **Expected:** Audience validation names exclusions; outputs retain scope/range; delivered attachment bytes are
  inspected; repeating an occurrence does not resend successful recipients.
- **Output branch:** Read the captured DashboardReportEmail HTML/text and optional inline image, then open the
  report link as the recipient. Compare captured attachment bytes with Open my report before and after a fixture
  data/access change: the latter renders again with current rights and does not retrieve the original sent file.
  Export status is delivery metadata, not report content. Include received versus owned/shared schedule views.
- **Recovery:** Invalid audience, disabled owner, unavailable renderer, failed send, unknown outcome and
  permission loss retain evidence and a bounded recovery action.
- **Screens/modes:** Reports, Schedules, ScheduledReportDetail, ReportDelivery, ownReportFile/exportAudit;
  System Jobs/MailQueue handoffs, TUI/Standard forms.

### DASH-10 — Evaluate an alert and manage its audience or contacts

- **Actor/start:** Rule author/recipient with local webhook/mail sinks; deterministic firing, no-data and
  recovered readings.
- **Objective/steps:** Alerts → create/copy AlertRule → configure basis/condition → select contacts or audience
  → Test/Evaluate → inspect own/received state and deliveries → silence/mute → resume/pause; edit shared item
  from AlertShared.
- **Expected:** Thresholds survive unrelated edits without numeric drift; evaluation request/skipped/result are
  distinct; notifications name firing/unknown/recovery and reach only allowed recipients.
- **Recovery:** Unavailable reading, invalid condition, shared item in use, failed contact, expired silence and
  canceled delete show the affected rule without disturbing unrelated rules.
- **Screens/modes:** Alerts, AlertRule, AlertShared and state/delivery tables; long configuration forms at 390,
  actual TUI/Standard edits.
- **Message branch:** Capture REPORT_ALERT for firing, recovered and no-data/error fixtures; inspect HTML/text,
  labels and per-recipient link. Opening the alert shows current state, which may differ from the notification's
  state at send time. Keep a technical contact-point notification separate from the checked business audience.

### DASH-11 — Share and play a playlist unattended

- **Actor/start:** Playlist owner/reader, two accessible dashboards and one inaccessible item; local
  wall/shared-device context if configured.
- **Objective/steps:** Playlists → create/select/share → Playlist play → move/pause → open WallDisplay → observe
  refresh → change accessible items → reopen as reader.
- **Expected:** Playback retains definition/range and item identity; filtering inaccessible items never selects
  the wrong original item; full-screen view does not expose author controls.
- **Recovery:** Removed dashboard, failed frame, revoked share, empty playlist and delayed prior-frame response
  recover without showing stale/private data.
- **Screens/modes:** Playlists, Playlist, WallDisplay and shared Device handoff; browser playback and truthful
  terminal/Standard list fallback.

### DASH-12 — Annotate a result and synchronize definitions

- **Actor/start:** Author with annotation authority, disposable definition folder and local Git remote; saved
  dashboard from DASH-06.
- **Objective/steps:** Annotations → add a dated note → inspect on the matching dashboard; Definition
  export/import/compare → GitSync configure/push → local counterpart change → pull → reopen affected definition.
- **Expected:** Annotation source/time is clear; sync reports which definitions changed and preserves a
  recoverable prior version; authored labels and filters survive export/import.
- **Recovery:** Invalid note scope, refused location, malformed import, local/remote conflict and failed sync
  retain independent successes and identify the failing item.
- **Screens/modes:** Annotations, GitSync, Definition/version history and Dashboard; local files/remotes only,
  printed/exported content and supported modes.

### DASH-13 — Reconcile access, embedded context and outputs

- **Actor/start:** Author, reader, aggregate-only and outsider using the earlier known-good records.
- **Objective/steps:** Open shared dashboard directly and through System/Monitor embed → filter/drill/export →
  compare scope; probe the selected helper/download with the outsider; inspect all remaining inventory branches.
- **Expected:** Direct and embedded links retain owning identity and range; denied users receive neither
  restricted rows nor stored files; each route has a recorded inspection or complete activity.
- **Recovery:** Changed audience, revoked dataset grant, missing saved definition and unavailable output device
  remain unresolved until exercised with valid prerequisites.
- **Screens/modes:** All inventory coverage reconciliation, not another full walk; framework ReportMounts
  fixture only for non-addon mount isolation.

## 5. Existing checks and future verification

This is an available-test map, not a results ledger. Read assertions and suite setup before selecting a run; static
shape checks, offline component fixtures, live browser walks and service rules provide different evidence.

| Component / suite | Existing specs | Coverage and limits |
| --- | --- | --- |
| tools / server | [ReportMounts](../runtime/components/base/tools/test/ReportMounts.spec.js) | Mount links/policy, dataset choices and sharing; not a live addon walk. |
| tools / server | [CalcGrid](../runtime/components/base/tools/test/CalcGrid.spec.js) | Document persistence, profile refusal, formulas/undo, collaborative edits and scheduled XLSX; not UI completion. |
| tools / acceptance client | [Monitoring](../runtime/components/base/tools/test/acceptance/client/Monitoring.spec.js) | Actual authoring/filter/embed/export and time controls, with later monitoring pages partly render-only; reuse selected assertions. |
| tools / acceptance client | [CalcGrid](../runtime/components/base/tools/test/acceptance/client/CalcGrid.spec.js) | Grid and Dashboards create/open/save/export, formula/format/undo and two-page editing; scope/refusal branches remain to review. |
| framework / server | [ReportAudience](../runtime/components/base/framework/test/ReportAudience.spec.js), [ReportDrill](../runtime/components/base/framework/test/ReportDrill.spec.js), [ReportAnnotations](../runtime/components/base/framework/test/ReportAnnotations.spec.js) | Audience equivalence, retry/unknown outcomes, aggregate/record scope and annotation rules; service evidence only. |

Future acceptance candidates: **DashboardsAuthoringWalk** (DASH-01–06, extending Monitoring where it already covers
the behavior), **ReportDeliveryWalk** (DASH-09), **ReportAlertsWalk** (DASH-10), **ReportPlaylistWalk** (DASH-11),
**ReportMaintenanceWalk** (DASH-12) and **ReportAccessWalk** (DASH-13). Extend CalcGrid for uncovered DASH-07–08
branches rather than create another spreadsheet golden path. Reuse existing server report fixtures without addon data.

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
| DASH-01 — Start from a trustworthy dashboard | exercised — partial | Author opened Monitor overview. Independent panels named unavailable datasets and retained their menus. A newly authored dashboard displayed its accessible rows. Star/recent, variables and restore remain pending. | DASH-B01 |
| DASH-02 — Explore a dataset and resume a saved search | exercised — partial | After fresh login, rdr-sales showed four group-scoped rows. Filtering relatedEnumId:north returned RDR_E1 and E2, saved as ui-b01-north. Split and changed-query reopen pending. Default dataset and time display findings 001/002. | DASH-B01 |
| DASH-03 — Explain a panel value down to allowed records | exercised — partial | Selected north from the owned dashboard table. Dashboard showed one filter and two northern rows. A table-cell click applies a filter. Dedicated Drill returned RDR_E3/E4 for Other. The aggregate-only panel returned counts and explained its source rows were restricted. Mixed-unit recovery pending. | DASH-B01 |
| DASH-04 — Create and steward a usable dataset | exercised — partial | John Doe opened the admitted rdr-sales steward detail, scope field, rebuild control and Used in section. Author opened its four-row preview and the generic new dataset definition editor. Creating a separate DataDocument, invalid fields, admit/retire and rebuild pending. | DASH-B01 |
| DASH-05 — Build an analysis and a reusable panel | exercised — partial | Author selected rdr-sales, Count of records and relatedEnumId in Analysis. Run returned the populated form without a result or explanation (003). Library listed rdr-panel and rdr-totals-panel. Explore independently created a working panel. Scoped author ran both counts, saved the grouped panel and verified the reopened library preview. | DASH-B01; DASH-B02 |
| DASH-06 — Assemble, share and revise a dashboard | exercised — partial | Saved dashboard ui-b01-dashboard, reopened its initial version, then added the saved Explore query and a CalcGrid panel. Shipped definitions remained distinct. Version 4 shared the dashboard with RDR_READER; the reader opened two northern rows in Standard. Opened comparison of versions 1 and 4. Restore remains pending. | DASH-B01 |
| DASH-07 — Create a spreadsheet over admitted data | exercised — partial | Created B01 Calc review over Enumeration with report author’s narrow profile, added Sheet1, entered =2+3 and saved. Exported XLSX contains formula and cached value 5. Formatting, undo and stale refresh pending. | DASH-B01 |
| DASH-08 — Edit a shared spreadsheet and consume its range | exercised — partial | Author shared B01 Calc review with RDR_READER. Reader reopened Sheet1 showing 5. Author saw reader presence and added Sheet1!A1 to the owned dashboard. CSV contains header 5 and no data rows, as described in the range dialog. Concurrent editor conflict/undo and revocation pending. | DASH-B01 |
| DASH-09 — Schedule a report and resolve individual delivery outcomes | exercised — partial | Reports provided functional workbook and dashboard downloads. Personal PDF schedule 100000 refused no-address, then persisted after address setup. Send now → occurrence 100000, one render and one failed personal recipient. See B01 for retry outcome. Multi-dashboard composition and mixed group runs pending. | DASH-B01 |
| DASH-10 — Evaluate an alert and manage its audience or contacts | exercised — partial | Created ui-b01-count over rdr-sales, opened explicit Evaluate now confirmation, observed last evaluated and Normal. History and delivery outcomes were separately visible. The rule was paused and showed Activate. Audience changes and firing/recovery remain pending. | DASH-B01 |
| DASH-11 — Share and play a playlist unattended | exercised — partial | Created ui-b01-playlist/B01 rotation from B01 Review Dashboard. Playback offered Previous/Next, one dashboard, source rows and its CalcGrid panel. Group and signed-out playback, kiosk recovery and multi-item rotation pending. | DASH-B01 |
| DASH-12 — Annotate a result and synchronize definitions | exercised — partial | The author received the scoped refusals. John Doe added an annotation and pushed one owned definition to the local Git folder. Pull recognised its file as unchanged and separately refused a private definition. Sync was disabled. Changed remote content and conflict recovery remain open. | DASH-B01; DASH-B02 |
| DASH-13 — Reconcile access, embedded context and outputs | exercised — partial | Compared steward dataset detail against author preview; tested the shared workbook as reader; inspected XLSX and CSV download bytes. Standard reader opened the shared dashboard and the author launched Dashboards by keyboard in the browser terminal. Aggregate-only drill hid individual rows. Outsider, embedded mounts and additional TUI controls remain pending. | DASH-B01 |

### Findings register

Use `DASH-UI-001` onward. P1 blocks work or essential understanding; P2 causes substantial friction or misleading
presentation; P3 is a localized refinement. Label evidence as observed behavior, static suspicion or design judgment.

| ID / severity | Actor, activity step, route, mode / viewport | Reproduction and impact | Evidence / owning layer | State / correction / retest |
| --- | --- | --- | --- | --- |
| DASH-UI-001 / P2 | Author, DASH-02, Explore, Material | With only CalcGrid range available, open Explore. The preselected service dataset immediately reports “a service dataset returns frames; run it as a report query”, then tells the reader to correct the query. The input is empty and no eligible alternative was offered. A reader needs a supported starting source or an actionable route to run a report query. | B01 03-default-explore.jpg, Explore default selection and error recovery. | **Fixed and checked 2026-11-25.** `list#Datasets` and `get#Dataset` now mark a service dataset (`service`). Explore leaves service datasets out of its choices and its default. With only service datasets it links to Analysis. A service dataset asked for by name shows that it returns report results, with the same link, instead of "correct the query". ReportDatasetServices checks the flag. |
| DASH-UI-002 / P2 | Scoped author, DASH-02, Explore, Material | Filter rdr-sales by relatedEnumId:north. Both Nov 25 fixture records show 1970-01-01T00:00:00.000Z. Their surrounding-entry links carry a Nov 25, 2026 anchor and the exported _timestamp matches that anchor. The result time should use the dataset’s configured time field. | B01 06-filtered-records.jpg and dashboard-csv.zip; Explore.xml builds timestampText from four fixed names, while anchorTime uses the dataset timeField. | **Fixed and checked 2026-11-25.** Record time comes from the dataset's time field, as the surrounding-entry anchor does. It is shown in the viewer's zone, with the zone named once; an export keeps UTC text, and a record without the field shows no time rather than 1970. ExploreHistogram checks the shown time. Live: Explore listed otel_logs records in US/Central with milliseconds. |
| DASH-UI-003 / P1 | Scoped author, DASH-05, Analysis, Material | Select rdr-sales → Count of records → Group by relatedEnumId → Run. The URL contains dataset, aggregate, grouping and range. The form redisplays without a result, error or Save panel action. The same happened for ungrouped count. The four source rows are visible in DatasetView. | B01 07-run-without-result.jpg. The Analysis screen expects the run submit parameter and ReportAnalysis.view gates evaluation on it. The observed URLs omitted it. | **Fixed and checked 2026-11-25.** The cause was in the shared client. `m-form-link` read the submit button from the focused element, and a pressed QBtn moves the focus to its inner helper span, so no form to a screen sent its button name. It now records the clicked button, as `m-form` does; WebrootVue.qvt checks a mouse click. The ungrouped count also had no row, because the search engine has no bucketless count; it now shows the answer's source count as one row. Live: grouped otel_logs Run showed INFO 2719, WARN 13, ERROR 2, and ungrouped showed All records, count 2744, both with Save analysis. A grant-scoped rdr-sales author was not rechecked. |
| DASH-UI-004 / P3 | Administrator, DASH-12, Git sync, Material | Add local database-backed example-wiki with folder UI review and directory review-b01, then Push now. Last push displays a raw 13-digit epoch (1795607783560), making the time of the last sync difficult to read. Format the time in the viewer’s zone. GitSync.xml displays the returned numeric date directly. Unlike 002, this surface has the correct timestamp and needs presentation formatting. | B01 20-git-sync-times.jpg, GitSync list rendering. | **Fixed and checked 2026-11-25.** Last push and Last pull are shown in the viewer's zone, with the zone named once, and "Never" when empty. Live in DASH-B02: the page named US/Central and showed 2026-11-25 19:06:09 for both push and pull. |
| DASH-UI-005 / P3 | DASH-05, scoped author, Material Definition version history | Changed Date shows 1795655096257 and 1795655163876 for versions 1 and 2. The author cannot read when either version was saved. | Definition.xml displays changedDate without formatting. See panel-folder.json. | **Fixed 2026-11-25.** `versions#Definition` answers each time stamp as epoch-millisecond text, which the history displayed as it came; Changed Date now has a date format, so it is shown as a date in the viewer's zone, with the zone named once. DefinitionVersions checks the shown date. Log context's times, raw date text or epoch milliseconds from the hit, are now shown the same way, as Explore shows them; the Rust monitor_readers (html) test asserts the Time cell is the fixture's ISO time in john.doe's zone (`yyyy-MM-dd HH:mm:ss.SSS`) with "Times shown in America/Chicago.". Not rechecked live. |

### Bounded batch record

For each later batch record:

- Intent, activities and branch limits; runtime/build/components, identities/grants, fixture IDs and dates.
- Observed steps and outcomes; evidence filenames, console/network/output observations and findings.
- Workarounds and blocked/deferred branches, with prerequisites and follow-up.
- In refinement only: decisions, owning files/components, correction, focused verification commands/results and retest.
- Temporary settings/grants/files/jobs restored, remaining cleanup and companion handoffs.

### DASH-B01 — Framework report author and reader, 2026-11-25

Current maximal release at `6d8bb49f8a7c`, fresh six-component runtime on local port 18765. Browser hosts dashauthor, dashreader and dashadmin separated identities. The author and reader held RDR_A and RDR_READER. The author used the review-enum CalcGrid profile. Refreshing the author’s sign-in after fixture setup exposed rdr-sales and rdr-totals. John Doe supplied the separate steward control. The 13 ledger rows above describe the steps walked. No regression or acceptance suite was used as review evidence.

**Artifacts and state.** Evidence is under ignored `plans/ui-refinement-evidence/dashboards/B01/`. The author saved `ui-b01-north`, `ui-b01-dashboard`, `ui-b01-playlist` and `calc-b01-calc-review-18eb4714`. The report preview downloads returned HTML with embedded download links. The decoded XLSX had Enumeration and Sheet1; Sheet1 A1 stored formula 2+3 and value 5. The dashboard CSV bundle held the two northern rows and a one-cell Calc panel filename. The expected first-row-as-heading rule explains why the one-cell range has no data rows in the dashboard. The recipient detail for schedule 100000 recorded the first delivery as failed. The server identified an empty From on DASHBOARD_REPORT; the original failure produced no SMTP receipt.

**Delivery recovery.** The original failed run correctly refused Retry after pausing had changed its schedule revision. The coordinator set the synthetic From address on DASHBOARD_REPORT and its local relay. A fresh occurrence after resuming showed Complete / 1 sent. The loopback sink captured the expected subject and review.report.author@review.invalid with one 72,865-byte PDF attachment (delivered-report.pdf). The schedule was paused again.

**Rechecks and modes.** On a fresh signed-in hostname, Analysis Run again navigated to the grouped query URL without a run parameter. After network idle and disappearance of the progress indicator, no result, failure or Save panel control appeared (21-fresh-analysis.jpg). The dedicated Drill screen reported each dataset’s access correctly. The Other bucket showed E3 and E4; rdr-totals-panel exposed count 2 for each bucket and explained that the source records were restricted. Additional evidence covers the 390px Analysis form, 1920px dark appearance, the Standard reader dashboard, and the author’s keyboard-opened Dashboards list in the browser terminal. Modal, query-race, multieditor, provider and access branches not individually recorded in the ledger remain pending.

### DASH-B02 — November 25 continuation

As review.report.author in Material, Run counted four admitted rows. Grouping returned north 2, east 1 and south 1. Ungrouped returned All records 4. Saved “Closeout regional counts,” reopened the versioned definition from Library panels and previewed it. The preview had the same three values and an Other count of zero. The fifth fixture record belongs to RDR_B. This resolves the original scoped-author question for DASH-UI-003. See `analysis-grouped.json`, `analysis-ungrouped.json`, `panel-reopened.json` and `panel-values.json`.

Git sync initially showed Never for both timestamps. The scoped author put the saved panel in the UI review folder. As John Doe, Push now wrote zero files and Pull now identified the panel JSON under `git://example-wiki/review-closeout/` and explained that the sync user could not read the private definition. The page named US/Central and showed 2026-11-25 19:06:09 for both operations. The timestamp fix DASH-UI-004 passed. The sync was disabled. This first attempt did not exercise an accessible definition. See `sync-added.json`, `sync-push.json`, `sync-pull.json` and `sync-disabled.json`.

In the same folder John Doe added a definition he owned. Push wrote one file. Pull recognised it as unchanged and separately refused the private panel. The operator disabled sync. A changed-file import is still open. See sync-positive-push.json, sync-positive-pull.json and sync-final-disabled.json. Calc exercised two pages editing and discarding a shared change, formula entry, formatting and Undo. See walk-checks.txt.

Local evidence is under `plans/ui-refinement-evidence/closeout-d10e22f/`.

#### Remaining branches

The ledger records the strongest observed outcome. The remaining steps, recovery paths and modes in each section 4 activity are deferred to the fixture and next check below. An explicitly blocked or failed result keeps that status.

| Activity | Required setup and next observation |
|---|---|
| DASH-01 | Resume a dashboard under a reader after its default or access changes. |
| DASH-02 | Reopen a saved search as a different scoped reader and recover from a removed field. |
| DASH-03 | Drill a selected aggregate to its admitted records under both reader profiles. |
| DASH-04 | Revise a disposable dataset and verify an existing panel after a rejected schema edit. |
| DASH-05 | Reopen the panel as a reader with a distinct grant and compare the saved definition after revision. |
| DASH-06 | Use two editors to provoke a stale save, restore a prior version and verify the shared reader. |
| DASH-07 | Refresh a saved spreadsheet after its underlying permitted rows change. |
| DASH-08 | With two editors and a shared range consumer, revoke the grant and retry stale edits. |
| DASH-09 | Capture scheduled delivery for two recipients with distinct grants and verify each attachment. |
| DASH-10 | Trigger and clear a fixture alert and inspect each eligible contact's actual delivery. |
| DASH-11 | Play a multi-dashboard playlist as a reader after one item loses its grant. |
| DASH-12 | Change an accessible Git definition on the remote, pull it and compare its persisted body. |
| DASH-13 | Repeat the same export through the standalone dashboard and embedded consumer under a scoped reader. |

## 7. Decisions

- All 27 Dashboards XML definitions, including LogContext and Split helpers, plus two framework report email
  outputs belong here. Monitor wrappers retain their own context checks in the Monitor plan.
- Technical reporting and framework-only scoped fixtures are sufficient; no addon application is loaded.
- IDs are `DASH`; definition/dataset mutation, distribution and reader workflows are separate activities, so a render
  of Reports or Alerts cannot stand in for delivery or evaluation.

- Preparing or auditing these plans changes documentation only. Application/API changes, test runs, runtime walks
  and fixture creation belong to separate review/refinement execution.
- Current code, mounts and grants take precedence over older plans. Historical outcomes, permissions and exceptions
  are not inherited. Preserve routes, stored preferences and public contracts in any later refinement.
- Finish a review with explicit branch outcomes and a candid unresolved ledger; missing prerequisites do not become
  passes. Building absent features is outside this review.
