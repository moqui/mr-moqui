# MR Moqui — UI review: Dev AIDE artifact authoring

**Review update — 2026-11-25.** DEVA-B02 records the latest live evidence. The activity ledger identifies the branches still open.

**Status — 2026-11-25, refinement.** Both DEVA-B01 findings are fixed (DEVA-UI-001/002).

**Previous status — 2026-11-25.** First live review, DEVA-B01. All fourteen activities have branch-level outcomes. A saved entity field propagated through service inputs to a persisted row in Tools Grid. Two P2 findings remain open.

Contents: [scope](#1-purpose-scope-and-sources) · [method](#2-review-method-and-refinement-criteria) ·
[inventory](#3-screen-and-surface-inventory) · [activities](#4-end-to-end-activities) ·
[verification](#5-existing-checks-and-future-verification) · [record](#6-coverage-findings-and-running-record) ·
[decisions](#7-decisions).

## 1. Purpose, scope and sources

Review the guided development loop from a discovered artifact to a deliberate, recoverable change: understand its
references and ownership, edit the model/service/screen, inspect the generated result, simulate or preview, save and
verify the consuming behavior. Browser canvases and actual terminal editors are distinct interaction surfaces.

Sources: [Dev AIDE README](../runtime/components/dev/dev-aide/README.md), [design](design-dev-aide.md),
[Autodoc](../docs/dev/Autodoc.md), [TUI](../docs/ui/TUI.md),
[application practices](../docs/dev/Practices-App-Dev.md), current Artifacts/Entities/Services/Screens/Eca/Data screens,
`GuidedEdit.qvue`, `Artifacts/Browse.qvue`, `devstatic` editor modules, `GuidedOps.js` and corresponding service/spec
sources. The [archived UI review](archive/UI-Refinement.md) leaves live model/sim/design and mode work open.

### Ownership and handoffs

| Surface | Owner / handoff |
| --- | --- |
| Artifacts, Entities, Services, Screens, Eca, Data and their children | This plan |
| GuidedEdit QVue reused by entity/service/screen editors, and Browse QVue artifact graph | This plan; each editor's real panes/dialogs are listed below |
| Workspace XML endpoints and static delivery | [Workspace](ui-review-dev-aide-workspace.md) owns source/host inventory; this plan exercises authoring endpoints through their consumer |
| File/Git/task/publish/group server and Assist session worktrees | Workspace; this plan hands off saved artifacts/diffs rather than repeat collaboration |
| Entity/service data inspection and generic schema maintenance | [Tools](ui-review-tools.md); guided modeling, simulation and preview remain here |
| Shared editor/focus/terminal behavior | [Shared/Gallery](ui-review-shared-gallery.md), verified again with these real editors |

Edits of upstream fixture artifacts use a separate site extension root. Do not edit source-linked framework code,
and do not build absent terminal/browser parity merely to finish a walk.

## 2. Review method and refinement criteria

Apply the [common review protocol](ui-review-shared-gallery.md#common-review-protocol) and
[verification policy](ui-review-shared-gallery.md#verification-policy-for-later-execution). The details below add this
area's fixtures, dependencies and judgment criteria.

### Environment, identities and records

- Reuse the Workspace plan's proposed viewer/extender/full identities and recorded roots. Prepare a small local
  upstream fixture component plus a distinct site component, both disposable: entity with relationships/index/view,
  seed data, XML Actions service with nested parameters/SECA, and screens with named extension anchors.
- Include one broken extension, missing artifact, stale disk edit, invalid field/type, harmless table difference,
  invalid service input and buffered screen error. Record fixture names and locations; comparisons check both site
  output and the untouched upstream fixture. Keep a separate database for destructive DDL planning/execution.
- Terminal guided edits use server-side per-user/per-artifact buffers with a two-hour idle lifetime. Browser
  guided views edit the Workspace tab and companion buffers, with local draft recovery; they do not share that
  terminal buffer lifecycle. Canvas preview tokens are a third, temporary server-side buffer. Record each kind
  separately and discard/save deliberately. A live preview uses submitted unsaved text, not a silently saved file.
- Service simulation must leave fixture business rows unchanged while returning proposed rows/results/logs; some
  file/Git/Dev AIDE writes are intentionally refused in sim. Save-as-spec writes only the fixture repository and Run
  spec remains the editor's local simulation action, not authorization for repository-wide test execution.
- Docs export uses a writable disposable root. Browser design canvas, schemas/completion, terminal components and
  devtools binary are checked as prerequisites; an unavailable feature stays an explicit branch. Publish/restart is
  sequenced through Workspace after authoring verification.

### Area-specific criteria

- Artifact identity, source path/line, ownership/origin and incoming/outgoing references stay clear across graph,
  docs, model, editor and live consumer. A missing artifact is not an empty successful view.
- Structured controls and generated XML/JavaScript agree. Name/type choices are discoverable, selected references
  survive paging, and invalid changes preserve editable input and precise problems.
- Buffered, saved, reloaded, table-applied, simulated and published are different states. Extension-mode operations
  must target the site, with meaningful placement and preview before persistence.
- Terminal twins preserve supported edits, nested paths, action identity and pending counts. Unsupported actions
  explain browser delegation; a rendered twin is not proof that its Save/conflict/preview flow works.

### Ordered waves

| Wave | Activities | Continuation |
| --- | --- | --- |
| 1. Understand the artifact | DEVA-01–03 | Search → graph/docs → extension ownership |
| 2. Model data | DEVA-04–07 | Fields/relations/views → schema plan → seed records |
| 3. Write behavior | DEVA-08–10 | Parameters/logic/ECA → simulation/spec |
| 4. Build presentation | DEVA-11–12 | Screen tree → canvas/extension/preview |
| 5. Save and reconcile | DEVA-13–14 | Buffered conflict/recovery → live consumer and remaining modes |

The entity supplies the service and screen fixtures, so save/relink it before their valid preview runs. Keep
unsaved/conflict fixtures separate from the source used for later successful paths. Destructive table changes are
isolated after read/schema comparison; Git/publish handoff waits until intended generated files are inspected.

## 3. Screen and surface inventory

This inventory accounts for **22 XML definitions** assigned to this file; screen containers and helper
definitions are included. Routes below are Material paths unless labelled otherwise; app routes have Standard
`/apps` counterparts. A file's physical presence does not prove access or a supported interaction in every mode.
Each row maps to §4; helper inspection does not replace its parent workflow. Counts describe definitions, not user
activities. Dialogs, templates and non-XML surfaces follow the route tables.

### dev-aide: `DevAide`

| Route / surface | XML source | Default / variants / boundary | Activities |
| --- | --- | --- | --- |
| `/qapps/dev/Artifacts` | [Artifacts.xml](../runtime/components/dev/dev-aide/screen/DevAide/Artifacts.xml) | Default: `Search`. | DEVA-01, DEVA-02, DEVA-03 |
| `/qapps/dev/Data` | [Data.xml](../runtime/components/dev/dev-aide/screen/DevAide/Data.xml) | Context: `filter`, `type`, `component`. | DEVA-07 |
| `/qapps/dev/Eca` | [Eca.xml](../runtime/components/dev/dev-aide/screen/DevAide/Eca.xml) | Context: `kind`, `component`, `target`, `when`. | DEVA-09 |
| `/qapps/dev/Entities` | [Entities.xml](../runtime/components/dev/dev-aide/screen/DevAide/Entities.xml) | Default: `List`. | DEVA-04 |
| `/qapps/dev/Screens` | [Screens.xml](../runtime/components/dev/dev-aide/screen/DevAide/Screens.xml) | Default: `Tree`. | DEVA-11 |
| `/qapps/dev/Services` | [Services.xml](../runtime/components/dev/dev-aide/screen/DevAide/Services.xml) | Default: `List`. | DEVA-08 |

### dev-aide: `DevAide/Artifacts`

| Route / surface | XML source | Default / variants / boundary | Activities |
| --- | --- | --- | --- |
| `/qapps/dev/Artifacts/Browse` | [Browse.xml](../runtime/components/dev/dev-aide/screen/DevAide/Artifacts/Browse.xml) | Companions: [Browse.qvue](../runtime/components/dev/dev-aide/screen/DevAide/Artifacts/Browse.qvue). Authored text: `qvue`, `tui`. Context: `artifactId`, `text`. Declared modes: `qvue,tui`. | DEVA-02 |
| `/qapps/dev/Artifacts/BrowseTui` | [BrowseTui.xml](../runtime/components/dev/dev-aide/screen/DevAide/Artifacts/BrowseTui.xml) | Authored text: `tui`. Hidden/helper entry; enter through its parent action and inspect direct access. Context: `artifactId`, `text`. | DEVA-02 |
| `/qapps/dev/Artifacts/Customizations` | [Customizations.xml](../runtime/components/dev/dev-aide/screen/DevAide/Artifacts/Customizations.xml) | Inspect the route and its visible controls through the activity. | DEVA-03 |
| `/qapps/dev/Artifacts/Doc` | [Doc.xml](../runtime/components/dev/dev-aide/screen/DevAide/Artifacts/Doc.xml) | Context: `artifactId`, `includeSource`. | DEVA-02 |
| `/qapps/dev/Artifacts/DocExport` | [DocExport.xml](../runtime/components/dev/dev-aide/screen/DevAide/Artifacts/DocExport.xml) | Inspect the route and its visible controls through the activity. | DEVA-02 |
| `/qapps/dev/Artifacts/Refs` | [Refs.xml](../runtime/components/dev/dev-aide/screen/DevAide/Artifacts/Refs.xml) | Context: `artifactId`. | DEVA-01 |
| `/qapps/dev/Artifacts/Search` | [Search.xml](../runtime/components/dev/dev-aide/screen/DevAide/Artifacts/Search.xml) | Context: `text`, `kind`, `component`. | DEVA-01 |

### dev-aide: `DevAide/Entities`

| Route / surface | XML source | Default / variants / boundary | Activities |
| --- | --- | --- | --- |
| `/qapps/dev/Entities/Edit` | [Edit.xml](../runtime/components/dev/dev-aide/screen/DevAide/Entities/Edit.xml) | Authored text: `qvue`, `tui`. Hidden/helper entry; enter through its parent action and inspect direct access. Context: `artifact`, `tab`, `confirmRemove`. Declared modes: `qvue,tui`. | DEVA-04, DEVA-05, DEVA-06, DEVA-07, DEVA-13 |
| `/qapps/dev/Entities/EditTui` | [EditTui.xml](../runtime/components/dev/dev-aide/screen/DevAide/Entities/EditTui.xml) | Authored text: `html,qvt`, `tui`. Hidden/helper entry; enter through its parent action and inspect direct access. Context: `artifact`, `tab`, `confirmRemove`. | DEVA-04, DEVA-05, DEVA-06, DEVA-07, DEVA-13 |
| `/qapps/dev/Entities/List` | [List.xml](../runtime/components/dev/dev-aide/screen/DevAide/Entities/List.xml) | Context: `filter`, `component`, `kind`. | DEVA-04 |

### dev-aide: `DevAide/Screens`

| Route / surface | XML source | Default / variants / boundary | Activities |
| --- | --- | --- | --- |
| `/qapps/dev/Screens/Design` | [Design.xml](../runtime/components/dev/dev-aide/screen/DevAide/Screens/Design.xml) | Authored text: `qvue`, `tui`. Hidden/helper entry; enter through its parent action and inspect direct access. Context: `rootId`, `path`, `page`, `node`, `tab`, `previewQuery`. Declared modes: `qvue,tui`. | DEVA-12, DEVA-13 |
| `/qapps/dev/Screens/DesignTui` | [DesignTui.xml](../runtime/components/dev/dev-aide/screen/DevAide/Screens/DesignTui.xml) | Authored text: `html,qvt`, `tui`. Hidden/helper entry; enter through its parent action and inspect direct access. Context: `rootId`, `path`, `page`, `node`, `tab`, `previewQuery`. | DEVA-12, DEVA-13 |
| `/qapps/dev/Screens/Tree` | [Tree.xml](../runtime/components/dev/dev-aide/screen/DevAide/Screens/Tree.xml) | Authored text: `qvt`, `tui`. Context: `location`, `path`, `parentRootId`, `parentPath`. | DEVA-11 |

### dev-aide: `DevAide/Services`

| Route / surface | XML source | Default / variants / boundary | Activities |
| --- | --- | --- | --- |
| `/qapps/dev/Services/Edit` | [Edit.xml](../runtime/components/dev/dev-aide/screen/DevAide/Services/Edit.xml) | Authored text: `qvue`, `tui`. Hidden/helper entry; enter through its parent action and inspect direct access. Context: `artifact`, `tab`. Declared modes: `qvue,tui`. | DEVA-08, DEVA-09, DEVA-10, DEVA-13 |
| `/qapps/dev/Services/EditTui` | [EditTui.xml](../runtime/components/dev/dev-aide/screen/DevAide/Services/EditTui.xml) | Authored text: `html,qvt`, `tui`. Hidden/helper entry; enter through its parent action and inspect direct access. Context: `artifact`, `tab`. | DEVA-08, DEVA-09, DEVA-10, DEVA-13 |
| `/qapps/dev/Services/List` | [List.xml](../runtime/components/dev/dev-aide/screen/DevAide/Services/List.xml) | Context: `filter`, `component`. | DEVA-08 |

### Embedded surfaces, dialogs, outputs and reconciliation

| Guided editor / supporting surface | Activity / inspection |
| --- | --- |
| Artifacts Search/Refs, Browse QVue graph pan/zoom/filters/trail and BrowseTui Miller columns | DEVA-01/02; selected artifact links lead to the same identity |
| Docs diagrams/zoom/copy/package/root and DocExport repository/path | DEVA-02; source/doc metadata and actual exported files |
| Customizations origin/state and extension source navigation | DEVA-03; broken/unmatched extension remains visible |
| GuidedEdit reused entity pane: fields/kinds/picklist/status, relationships/incoming/indexes, view members/aliases, table, data/doc tabs | DEVA-04–07; shared file [GuidedEdit.qvue](../runtime/components/dev/dev-aide/screen/DevAide/GuidedEdit.qvue) has no route of its own |
| Entity EditTui pending header/Save/Discard/conflict, field/relation/index forms, plan/copy/type/DDL, seed add/edit/remove | DEVA-04–07/13; own paths and typed confirmation |
| Service settings/parameters/nested paths, action composer/palette/chips/generated JS, SECA and condition preview | DEVA-08/09; browser nested-parameter limitations are explicit, terminal Add child is separately exercised |
| Eca filters for SECA, EECA, EmECA and gitECA, priority/condition/source and replaced-rule rows | DEVA-09; active count and run order must distinguish a replaced definition from the effective rule |
| Try inputs/result/rows/log, snippet, Save as spec/Run spec and artifact Doc tab | DEVA-10; writes remain simulation and spec output is a fixture file |
| Screens Tree settings/create/move/remove, Design outline/properties/extension points/canvas/preview parameters/doc | DEVA-11/12; unsaved buffer preview and stamped source identity |
| Workspace `.qvue` Design/Preview, element outline/overlay, opening-tag edit, snippet insertion/move/remove, Text and terminal-twin display | DEVA-12; [DevAideQvue.js](../runtime/components/dev/dev-aide/screen/devstatic/DevAideQvue.js) implements a separate editor from XML screen design, using the same Workspace buffer and undo history |
| DesignTui outline/properties/intents/extension tree, add field/filter/action/widget/section/button, preview | DEVA-12/13; server-search service choices and JSON-safe native output |
| Workspace entityView/screenView/serviceView/xmlOp/schema/outline/refs, planDdl/runDdl, tryService/saveTrySpec/runSpec, canvasBuffer/stamps | These consumer interactions belong here; endpoint source inventory is DEVW-13 |
| `devstatic` entity/service/screen modeler modules | Static support, not extra app pages; loaded version must match source before diagnosing stale UI |

The current terminal editors stage changes and implement nested service parameters/extension operations that older
walks did not exercise. Do not assume the browser edits nested parameters because its terminal twin does. Sim does
not make Dev AIDE's own file/Git writes safe or available; refused operations are expected boundaries.

## 4. End-to-end activities

Use visible controls and inspect intermediate states, confirmation and the resulting data/output. Unless narrowed
below, each activity has Material light/dark, actual supported TUI and a brief Standard/output branch. Select narrow
and keyboard checks where layout/interaction differs. Record each unattempted branch in §6; a service call or render
alone does not complete the activity. Names beginning `review.` are fixture proposals, not shipped accounts.

### DEVA-01 — Find an artifact and follow its references

- **Actor/start:** Viewer with indexed fixture entities/services/screens/rules and a missing-name case.
- **Objective/steps:** Artifacts/Search → search by name/segment → open result/source line → Refs
  incoming/outgoing → select referenced artifact → definition → return via trail.
- **Expected:** Artifact kind, full identity, file and line agree; exact/partial matches and reference direction
  are understandable.
- **Recovery:** No match, stale index, deleted source, inaccessible root and broken reference explain the
  missing target without losing search.
- **Screens/modes:** Search, Refs, Workspace source handoff and palette; browser/TUI/Standard, keyboard result
  selection.

### DEVA-02 — Explore the graph and export useful documentation

- **Actor/start:** Viewer and permitted docs exporter; artifact with relationships/diagrams and writable fixture
  root.
- **Objective/steps:** Browse → pan/zoom/filter/recenter/trail → Doc → package/root/field link → diagram
  zoom/copy → DocExport choose root/path → inspect produced files.
- **Expected:** Graph direction, center and trail stay clear; docs describe the selected current artifact;
  export paths/content match selection and respect root policy.
- **Recovery:** Missing artifact, empty relation side, unsupported diagram, refused upstream export and partial
  export failure keep independent results and a useful return path.
- **Screens/modes:** Browse/BrowseTui, Doc, DocExport and editor Doc panels; actual Miller-column keys and
  exported Markdown/diagrams.

### DEVA-03 — Understand what the site customizes

- **Actor/start:** Extender and viewer; upstream fixture, site extension and deliberately unmatched/broken
  extension.
- **Objective/steps:** Customizations → select state/source → Refs/Browse → open artifact guided editor →
  inspect origin/extension mode → source diff → return.
- **Expected:** Site-owned change and upstream definition are distinct; unmatched extension explains its
  anchor/problem instead of silently disappearing.
- **Recovery:** Missing upstream source, read-only site root, invalid extension and changed origin leave the
  actual merged result clear.
- **Screens/modes:** Customizations, Refs, guided headers and Workspace source; browser/TUI/Standard supporting
  inspection before edits.

### DEVA-04 — Add and revise entity fields in the owning layer

- **Actor/start:** Extender on upstream fixture plus site root; full developer on own fixture; viewer refusal
  counterpart.
- **Objective/steps:** Entities/List → Edit inspect origins → add field by kind → edit/move/remove disposable
  field → create picklist/status with seed values → inspect generated changes → Save → reload model/consumer.
- **Expected:** Field type/name/origin and generated seed/extension files agree; upstream stays untouched in
  extension mode; saved model reflects the intended field.
- **Recovery:** Duplicate/invalid name/type, unsupported removal, stale source, denied tier and canceled discard
  preserve buffer and precise problems.
- **Screens/modes:** Entities/List/Edit, GuidedEdit and EditTui field dialogs; actual terminal operations,
  browser keyboard/narrow, Tools schema handoff.

### DEVA-05 — Model relationships, indexes and a view

- **Actor/start:** Author with saved fixture entities and related records.
- **Objective/steps:** Entity editor → relationship add/edit/remove → inspect incoming relations → index
  selection → view entity members/aliases → inspect generated SQL/data → docs diagram → save/reopen.
- **Expected:** Relationship direction/key mapping and index/view ownership are understandable; aliases produce
  intended columns/rows and references update.
- **Recovery:** Unknown related entity/field, duplicate alias/index, invalid join and failed validation keep
  edits recoverable and underlying model unmodified until save.
- **Screens/modes:** Entity relationship/index/view/table/doc panes, EditTui supported relation/index actions
  and Tools View/Schema handoff.

### DEVA-06 — Plan a table change and apply it deliberately

- **Actor/start:** Full developer and extender/viewer counterparts; isolated disposable database with known
  model/table difference.
- **Objective/steps:** Entity Table pane → compare → plan change/copy/type as supported → inspect SQL → cancel →
  plan again → type required table name → run → compare actual table/model.
- **Expected:** Safe relink/add-column behavior is distinct from destructive planned DDL; confirmation names the
  target and the result matches the plan.
- **Recovery:** Wrong typed name, forbidden tier, stale schema, unsupported backend and failed statement retain
  original evidence and explicit unresolved differences.
- **Screens/modes:** Entity editor Table and EditTui planDdl/runDdl/cancelDdl; actual local database only,
  TUI/Standard capability boundary.

### DEVA-07 — Edit seed records and verify loaded data

- **Actor/start:** Extender with site seed file and framework fixture entity, plus export-restricted
  counterpart.
- **Objective/steps:** Data → select file → guided data table → add/edit/remove row → Check against database →
  Save/load → inspect entity record → Export selected records into fixture file.
- **Expected:** Buffered file changes and loaded database records are distinct; row identity and entity type are
  clear; restricted credential data is not exported.
- **Recovery:** Bad field/value, mixed valid/invalid rows, failed load, stale seed file and canceled removal
  keep independent outcomes and original data explainable.
- **Screens/modes:** Data, entity data/seed panes, EditTui seed operations, Workspace loadChanged/exportRecords;
  actual fixture file/database readback.

### DEVA-08 — Define service inputs and outputs

- **Actor/start:** Author with XML Actions service and nested map/list parameters; viewer and extender
  counterparts.
- **Objective/steps:** Services/List → Edit settings → add/edit/remove input/output → required/default/type →
  nested parameter path using supported editor → inspect generated definition → save/reopen → Docs.
- **Expected:** Inputs versus outputs, nesting and defaults are clear; removal affects the selected path;
  declared outputs match intended contract.
- **Recovery:** Duplicate name/path, invalid type, unsupported browser nesting, stale source and denied edit
  preserve buffer without pretending parity.
- **Screens/modes:** Service Edit/GuidedEdit and EditTui Add child/Edit/Remove; actual terminal nested parameter
  path and browser supported subset.

### DEVA-09 — Compose logic and connect an ECA rule

- **Rule-browser branch:** use Eca's kind/component/target/when filters for SECA, EECA, EmECA and gitECA fixtures;
  inspect active count, priority/order and a replaced rule, then open its exact source line. Adding a SECA in the
  service editor is distinct from browsing these other rule kinds and does not imply a guided writer for each kind.
- **Actor/start:** Author with harmless service/SECA fixture and selectable called service; local simulated
  condition data.
- **Objective/steps:** Service actions palette → add/reorder/edit chips → inspect generated JavaScript → SECA
  add/select called service → Eca list → source/condition Try → save/reopen.
- **Expected:** Block order and generated action meaning agree; rule timing/condition/target and active status
  are clear; selected service label survives lookup paging.
- **Recovery:** Invalid attribute/expression, unknown target, unsupported action editing in terminal, failed
  condition and canceled rule keep existing behavior intact.
- **Screens/modes:** Service composer/SECA, Eca, EditTui service choices and read-only actions; generated
  JS/condition result as evidence.

### DEVA-10 — Simulate a service and retain a runnable fixture spec

- **Actor/start:** Viewer/author with valid/invalid inputs, harmless proposed entity write and forbidden Dev
  AIDE write service.
- **Objective/steps:** Service Try → fill inputs → run sim → inspect result/rows/logs → verify database
  unchanged → edit condition/snippet → Save as spec → inspect file → Run spec in sim.
- **Expected:** Proposed writes and actual persisted data are distinct; results/logs map to submitted inputs;
  generated spec captures meaningful expectations, not an unexplained pass.
- **Recovery:** Validation/service error, unauthorized data, refused file/Git write in sim, failed spec save and
  interrupted run preserve input and explain limits.
- **Screens/modes:** Service Edit/ EditTui Try/snippet/spec controls, Workspace authoring endpoints; fixture
  file only, no repository-wide test execution.

### DEVA-11 — Arrange the actual screen tree

- **Actor/start:** Author with disposable app root and child screens; extender counterpart for upstream
  structure.
- **Objective/steps:** Screens/Tree → expand mounted children → inspect settings → create child → move/order →
  open design → cancel then remove disposable child → inspect live navigation.
- **Expected:** Mounted route, file, menu order/default and source ownership remain distinguishable; generated
  links open the intended child.
- **Recovery:** Duplicate name, invalid parent, unsupported upstream move, missing child and canceled removal
  retain existing routes and explain limits.
- **Screens/modes:** Screens/Tree settings/new/move/remove dialogs and live fixture app; browser/TUI/Standard
  supported navigation.

### DEVA-12 — Design and preview a screen before saving

- **QVue branch:** Open a disposable same-name XML/`.qvue` pair in Workspace → Design → select a rendered
  element or outline row → edit its opening tag → insert a snippet before/after/inside → move/remove a sibling →
  undo/redo → Open in Text → Preview → Save/reopen. Verify selected source line, shared undo history and preview
  cleanup after switching files. Inspect the `.tui.json` companion display and a missing-twin placeholder; this is
  not a terminal visual designer. Use an invalid tag, failed frame and canceled edit to check retained source and
  truthful errors. DEVA-12 owns this editor even though DEVW-02 owns the surrounding Workspace tab.
- **Actor/start:** Author/extender with own screen and upstream fixture containing named anchors/forms.
- **Objective/steps:** Design → outline select widget → edit properties → add field/column/filter/row
  action/section/button → inspect extension points → preview unsaved buffer/parameters → inspect stamped source
  → Save → live render.
- **Expected:** Preview reflects the buffer, not disk; placement/ownership is clear; site extension changes do
  not rewrite upstream; generated XML and live behavior agree.
- **Recovery:** Missing anchor, invalid property/service, failed preview, conflicting source and canceled
  insertion retain current buffer and meaningful problems.
- **Screens/modes:** Design/GuidedEdit canvas/outline/properties/doc, DesignTui extension tree/intents/preview;
  actual terminal preview and browser keyboard fallback.

### DEVA-13 — Resolve buffered edits and a competing disk change

- **Browser/terminal distinction:** Run terminal pending/expiry/Save mine/Take disk on server edit buffers. In
  the browser, restore/discard an earlier local draft and save a visual view with dirty companion files. If one
  companion conflicts, inspect which earlier files saved, which remain dirty and the offered reopen/recovery;
  do not assume the tab's comparison dialog resolves every companion conflict. Preview-token expiry must not
  discard the browser's editable source. Coordinate tab persistence with DEVW-02/03.
- **Actor/start:** Two sessions on separate entity/service/screen fixture buffers; author, viewer and
  expired-buffer case.
- **Objective/steps:** Stage several edits → inspect pending count → navigate/reopen → change disk in other
  session → Save → compare → Take disk or Save mine on separate fixtures → Discard another buffer → inspect
  files in kind order.
- **Expected:** Per-user/artifact buffers stay isolated; conflict preserves both texts; Save reflects the chosen
  revision and reload order; expired buffer does not silently report a save.
- **Recovery:** Write failure between artifact kinds, second conflict, lost grant, canceled overwrite and idle
  expiry leave exact remaining changes and errors visible.
- **Screens/modes:** Entity/Service/EditTui and Screen/DesignTui pending/conflict controls plus browser
  equivalent; DEVW-03 owns ordinary file editor conflict.

### DEVA-14 — Verify the changed artifact through its consumer

- **Actor/start:** Author/viewer/extender and outsider using earlier saved fixture artifacts.
- **Objective/steps:** Follow Tools/entity/service or app Customize/Open in screen builder → inspect selected
  artifact → complete small allowed edit/sim/preview → return to live consumer → Docs/Refs → Workspace diff for
  publish handoff; cover remaining helpers.
- **Expected:** Context and ownership survive entry/return; merged model/service/screen works as reviewed;
  restricted tiers cannot invoke direct authoring endpoints to bypass UI.
- **Recovery:** Missing artifact/root, lost grant, stale buffer, unavailable devtools and unsupported render
  mode remain explicit unresolved branches.
- **Screens/modes:** All authoring routes, GuidedEdit/Browse/static modules and Workspace endpoints; one shared
  Dev AIDE sweep, no addon or real source mutations.

## 5. Existing checks and future verification

This is an available-test map, not a results ledger. Read assertions and suite setup before selecting a run; static
shape checks, offline component fixtures, live browser walks and service rules provide different evidence.

| Component / suite | Existing specs | Coverage and limits |
| --- | --- | --- |
| dev-aide / server | [ArtifactIndex](../runtime/components/dev/dev-aide/test/ArtifactIndex.spec.js), [ArtifactBrowser](../runtime/components/dev/dev-aide/test/ArtifactBrowser.spec.js), [ArtifactDocs](../runtime/components/dev/dev-aide/test/ArtifactDocs.spec.js) | Index/references, graph response and docs/export boundaries, with selected renders; not full navigation/editing. |
| dev-aide / server | [EntityModeler](../runtime/components/dev/dev-aide/test/EntityModeler.spec.js), [GuidedOps](../runtime/components/dev/dev-aide/test/GuidedOps.spec.js) | Merged origins/views, DDL planning/typed confirmation, export boundaries and operation generation; not live authoring completion. |
| dev-aide / server | [EntityEditTui](../runtime/components/dev/dev-aide/test/EntityEditTui.spec.js), [ServiceEditTui](../runtime/components/dev/dev-aide/test/ServiceEditTui.spec.js), [ScreenDesignTui](../runtime/components/dev/dev-aide/test/ScreenDesignTui.spec.js) | Buffered edits, seed load, nested parameters, conflicts and extension operations on throw-away roots; terminal interaction still needs execution. |
| dev-aide / server | [ServiceWriter](../runtime/components/dev/dev-aide/test/ServiceWriter.spec.js) | Simulation/result/logs, snippets/generated JS, rule condition and viewer-scope behavior. |
| dev-aide / client (within existing spec) | [DevAideScreen](../runtime/components/dev/dev-aide/test/client/DevAideScreen.spec.js), [DevAideQvue page assertions](../runtime/components/dev/dev-aide/test/client/pages/DevAideQvue.page.js) | The Screen spec also runs QVue template scanning/edit assertions; there is no separate current DevAideQvue spec. These offline checks do not establish live canvas, save or terminal-twin behavior. |
| dev-aide / client | [DevAideEntity](../runtime/components/dev/dev-aide/test/client/DevAideEntity.spec.js), [DevAideService](../runtime/components/dev/dev-aide/test/client/DevAideService.spec.js), [DevAideScreen](../runtime/components/dev/dev-aide/test/client/DevAideScreen.spec.js) | Offline editor fixtures; real source writes/reload/browser canvas need separate journeys. |

The [component README](../runtime/components/dev/dev-aide/README.md) identifies existing opt-in modeler/service/
screen-builder browser integration checks. Inspect their scope and reuse their scratch fixtures before adding
coverage; no cargo/test invocation belongs to document creation.

Future acceptance candidates: **DevAideArtifactWalk** (DEVA-01–03), **DevAideEntityWalk** (DEVA-04–07),
**DevAideServiceWalk** (DEVA-08–10), **DevAideScreenWalk** (DEVA-11–12), and the shared
**DevAideAccessTuiWalk** (DEVA-13–14 with DEVW-13). Keep generated test/spec output inside the fixture repository;
source operation vectors and offline editor tests are supporting evidence, not screenshots or another sweep.

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
| DEVA-01 — Find an artifact and follow its references | exercised · partial | Found RoundReviewAppItem by name, followed its reference list (12 incoming edges) and opened Workspace with the generated artifact selected. Keyboard palette and role variants pending. | B01 |
| DEVA-02 — Explore the graph and export useful documentation | exercised · partial | Opened graph and rendered documentation with a field table. Exported Entities for round-review-app into devw-git/review-docs. Verified the manifest and generated entity Markdown containing reviewNote. Diagram and alternate renderer checks remain open. | B01 |
| DEVA-03 — Understand what the site customizes | exercised · partial | Customizations reported zero site extensions and zero broken. A broken-extension repair and a populated extension comparison remain pending. | B01 |
| DEVA-04 — Add and revise entity fields in the owning layer | exercised · partial | Added reviewNote (text-medium) to the generated app, saved and reopened five fields. Disk XML confirmed the owner. Phone 390×844 explicitly displayed Read only below 1024 px; dialog localization and upstream extension branches pending. | entity-table.jpg; phone-readonly.jpg |
| DEVA-05 — Model relationships, indexes and a view | exercised · partial | Compared empty relationship and index lists, added ReviewNoteIdx on reviewNote, saved and verified an index in XML. View members, relationship creation and recovery pending. | B01 |
| DEVA-06 — Plan a table change and apply it deliberately | exercised · partial | Table initially showed four columns, then review_note text after Save reported one DDL statement. Verified the column in the generated table. Explicit type conversion and destructive rebuild confirmation pending. | entity-table.jpg |
| DEVA-07 — Edit seed records and verify loaded data | exercised · partial | Opened Seed data twice with a site seed file present. No rows, explanation or actions appeared. Data inventory exposed the site seed file. Via Data → Workspace → Entity, edited a populated seed fixture, saved and loaded one record. Tools Grid reopened DEVA_SEED_01 with the edited value. Starting directly from the empty guided tab remains blocked by DEVA-UI-001. Populated guided seed tab, keyboard edit, Save/load and independent Grid readback passed. | seed.jpg; DEVA-B02 |
| DEVA-08 — Define service inputs and outputs | exercised · partial | Filtered services by component, opened create#Item and inspected its four inputs and one output. The generated entity supplied reviewNote(auto) and the Try input. Editing nested Map/List and output changes pending. | B01 |
| DEVA-09 — Compose logic and connect an ECA rule | exercised · partial | Implementation displayed the create entity service call with context in/out. SECA rules showed zero and the Add a SECA rule in the site action. Logic composition and rule persistence pending. | B01 |
| DEVA-10 — Simulate a service and retain a runnable fixture spec | exercised · partial | Try accepted itemName and reviewNote. Initial Run in sim hit a PostgreSQL Unix socket path limit. After shortening the runtime path, simulation returned virtual ID -1 and a proposed create row. Confirmed Run for real in the scratch DB and inspected itemId 100000. A reusable simulation spec remains pending. | sim.jpg |
| DEVA-11 — Arrange the actual screen tree | exercised · partial | From /qapps opened the generated app tree. Items was the default, Item hidden, and both named their parent file and mount. New/move/remove and broken-reference recovery pending. | B01 |
| DEVA-12 — Design and preview a screen before saving | exercised · partial | Opened FindItem designer, switched Text → Screen → Preview and selected 390px, then 1280px and Dark. Preview reported denied access for FindItem. After its grant propagated the preview contained the generated consumer row. Selecting NewItemDialog in Outline exposed its properties. Replacing button-text failed to update the preview or source (DEVA-UI-002). QVue companion and terminal preview remain pending. Original button-text edit passed through enabled Save, unsaved canvas, disk, reopen and consumer. | screen-preview.jpg; DEVA-B02 |
| DEVA-13 — Resolve buffered edits and a competing disk change | exercised · partial | Staged conflictDemo via the guided field editor; changed the disposable XML on disk before Save. Compared both versions, took disk and reopened the five retained fields. Multi-file, terminal buffer expiry and overwrite branches pending. | guided-conflict.jpg |
| DEVA-14 — Verify the changed artifact through its consumer | exercised · partial | Confirmed the changed entity field reached the generated service and Tools Grid. Reopened the grid and read item 100000, Review fixture, Round 20261125. Material Open in Workspace preserved entity, root and file. After the seeded grants took effect, the generated FindItem screen showed both 100000 and DEVA_SEED_01. Verified the edited seed row through Grid and the edited dialog label through FindItem. | consumer-grid.jpg; DEVA-B02 |

### Findings register

Use `DEVA-UI-001` onward. P1 blocks work or essential understanding; P2 causes substantial friction or misleading
presentation; P3 is a localized refinement. Label evidence as observed behavior, static suspicion or design judgment.

| ID / severity | Actor, activity step, route, mode / viewport | Reproduction and impact | Evidence / owning layer | State / correction / retest |
| --- | --- | --- | --- | --- |
| DEVA-UI-001 · P2 | ADMIN, DEVA-07, Material Entities/Edit, 1280×720 | Open RoundReviewAppItem in the modeler with the site seed file present. Choose Seed data. Its selected tab shows an empty pane. Returning after reload has the same outcome. Explain which file or records apply and how to start when the file has no applicable rows. | Observed, seed.jpg. Entity modeler seed panel. | **Fixed and checked 2026-11-25.** The tab showed when the seed file existed, but its pane waited for the file to be read on an edit. It is now read when the tab is chosen, with a reading state and an error. The pane shows only this entity's records, always with its fields as columns. With no records it says so and offers Add row. Live: moqui.example.Example over my-site's empty seed file showed the empty state; Add row made a row with all nine field cells. Nothing was saved. DevAideEntity page test. |
| DEVA-UI-002 · P2 | ADMIN, DEVA-12, Material Screens/Design, generated FindItem | Choose Screen → Design → Outline → NewItemDialog. Replace “New item” in the button-text property, then leave the field with Tab. The field retains the new label, Save stays disabled, the canvas button keeps “New item”, and Text still shows the original XML. Repeated with a keyboard-typed replacement. Stage the property edit in the XML buffer, or explain why the change cannot be applied. | Observed, designer-property.jpg. Screen designer outline/property editor. | **Fixed and checked 2026-11-25.** Reproduced on a generated app's FindItem. The `xmlOp` edit applied to the buffer and Save was enabled, but the Properties panel kept reading the selected element from the outline as it was before the edit, so it showed the old value. The selection now follows to the element at its path in the new outline. Live: button-text "Add item" stayed in Properties and in the text after Tab, with Save enabled. The canvas preview was refused for lack of a grant on the recheck runtime, so the canvas was not rechecked. The reviewer's disabled Save was not reproduced. DevAide page test. |

### Bounded batch record

For each later batch record:

- Intent, activities and branch limits; runtime/build/components, identities/grants, fixture IDs and dates.
- Observed steps and outcomes; evidence filenames, console/network/output observations and findings.
- Workarounds and blocked/deferred branches, with prerequisites and follow-up.
- In refinement only: decisions, owning files/components, correction, focused verification commands/results and retest.
- Temporary settings/grants/files/jobs restored, remaining cleanup and companion handoffs.

#### DEVA-B01 — 2026-11-25, generated app and site

Reviewed at checkout 6d8bb49f8a7c on the isolated dev runtime, port 18765, Chicago. DEVW-B01 supplied round-review-app and round-review-site. After the dependency correction, the site loaded. It remained the first authoring target, with the scratch Git and app repositories configured as additional site roots. John Doe (ADMIN) used the built-in browser at measured 1280×720 and a second editor at measured 390×844. The latter displayed an explicit read-only width boundary. Designer preview sizes and theme choices are distinct from the outer viewport.

The modeler saved reviewNote and ReviewNoteIdx to entity/RoundReviewAppEntities.xml. Its Table tab showed review_note text. The generated create service exposed reviewNote without a separate input definition. Simulation failed because its Unix socket path was 113 bytes; the service reported the required limit of 108. On explicit confirmation the real fixture write returned itemId 100000. Tools Grid reopened that row with the submitted field. After coordinating a shorter runtime path and restart, Run in sim showed its proposed row with virtual ID -1, and the generated FindItem consumer listed 100000 and DEVA_SEED_01. The real item and new field were independently visible in Tools. Extra evidence includes sim-short.jpg, seed-grid.jpg, generated-consumer.jpg and export.jpg. No product source changed and no automated suites ran.

Local evidence is under `plans/ui-refinement-evidence/deva-20261125/`. The original generated source, reviewed writes and short conflict marker are confined to the disposable repositories. Actor variations, full guided terminal operations and recovery branches beyond the recorded steps remain pending.

### DEVA-B02 — November 25 continuation

The authoring runtime marked the two copied components as site-owned. As John Doe, the entity modeler opened the existing DEVA_SEED_01 in the guided Seed data tab. After resolving a draft-recovery prompt, a keyboard edit changed reviewNote. Save reported one loaded record and Tools Grid showed “Seed via original guided tab” on the same ID. See `seed-inputs.json`, `seed-staged.json`, `seed-save-feedback.json` and `seed-final-grid.json`.

FindItem’s designer opened the generated consumer in its canvas with the fixture grant. Editing NewItemDialog’s button-text to “New review item” enabled Save, and the unsaved canvas displayed it. The saved XML, reopened Properties and the consumer all carried that value. The disabled Save symptom did not recur. See `designer-staged.json`, `canvas-before-save.json`, `property-reopened.json` and `designer-consumer.jpg`. Both original findings now have live evidence on the populated fixture.

Eight guided/TUI assertions failed with the fixtures under target. Workspace reported that the target directory was build output. With the copied components outside target, all eight passed (area-checks.txt and area-retry.txt). **Fixed 2026-11-25:** the guard now exempts the loaded runtime and the loaded components when they sit in a root's `target` (a test run links the components into a scratch runtime elsewhere, so the components are what it sees), with their own `target/` and the root's other build output still refused (dev-aide `confine_tests.rs`: `a_runtime_under_a_root_s_target_is_not_build_output`, `a_component_loaded_from_under_a_root_s_target_is_not_build_output`, `build_output_beside_or_inside_that_runtime_stays_refused` and `a_symlink_from_that_runtime_into_build_output_is_refused`).

Local evidence is under `plans/ui-refinement-evidence/closeout-d10e22f/`.

#### Remaining branches

The ledger records the strongest observed outcome. The remaining steps, recovery paths and modes in each section 4 activity are deferred to the fixture and next check below. An explicitly blocked or failed result keeps that status.

| Activity | Required setup and next observation |
|---|---|
| DEVA-01 | Use the keyboard palette under View and Edit tiers and return from a cross-reference. |
| DEVA-02 | Export a graph with related artifacts and compare its diagram and alternate rendering. |
| DEVA-03 | Change an extension independently of its upstream artifact and inspect the composed consumer. |
| DEVA-04 | Save a field change in a site extension and verify the merged model as a lower-tier reader. |
| DEVA-05 | Create a related view with a unique index and query it against seeded rows after reload. |
| DEVA-06 | Apply a deliberate schema change to an isolated database and inspect the retry and data outcome. |
| DEVA-07 | With file and database now deliberately out of step, retry a guided load and inspect the rejected row. |
| DEVA-08 | Evolve a service contract and call it with a valid and invalid input from its consuming screen. |
| DEVA-09 | Trigger the authored ECA rule from its real operation and verify its single side effect. |
| DEVA-10 | Persist a simulated service fixture, rerun its saved spec and compare the failure path. |
| DEVA-11 | Move or remove a node in a copied screen tree and verify navigation and undo after reload. |
| DEVA-12 | Repeat the original canvas action without preview permission and restore the grant for the same actor. |
| DEVA-13 | Use two writers on one saved artifact and reconcile the retained buffer with reopened disk state. |
| DEVA-14 | Follow the generated component through reload with a scoped non-author consumer. |

## 7. Decisions

- IDs are `DEVA`. Workspace retains file/Git/collaboration ownership; this file owns guided artifact semantics and
  the real editor surfaces, including templates with no standalone route.
- Upstream/site copies are both disposable; extension success requires inspecting which file changed and the live
  merged result. Destructive DDL uses a separate fixture database.
- Browser/terminal asymmetries are documented and exercised where supported, not converted into new-feature scope.

- Preparing or auditing these plans changes documentation only. Application/API changes, test runs, runtime walks
  and fixture creation belong to separate review/refinement execution.
- Current code, mounts and grants take precedence over older plans. Historical outcomes, permissions and exceptions
  are not inherited. Preserve routes, stored preferences and public contracts in any later refinement.
- Finish a review with explicit branch outcomes and a candid unresolved ledger; missing prerequisites do not become
  passes. Building absent features is outside this review.
