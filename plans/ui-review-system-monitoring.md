# MR Moqui — UI review: System monitoring and recovery

**Review update — 2026-11-25.** MON-B03 records the latest live evidence. The activity ledger identifies the branches still open.

**Status — 2026-11-25.** The [MON-B02 browser recheck](#mon-b02--post-cleanup-recheck-2026-11-25) confirmed both historical fixes, including the fingerprint fallback for a group with blank message and type. The System readiness comparison is recorded under SYS-UI-001.

**Original review — 2026-11-16:** review pass started. MON-B01 exercised the initial diagnostic navigation and error-recovery
branches, recorded two open findings and cross-linked one System finding. Every activity remains incomplete;
unrecorded branches and modes are pending. No refinement or automated test results are claimed.

Contents: [scope](#1-purpose-scope-and-sources) · [method](#2-review-method-and-refinement-criteria) ·
[inventory](#3-screen-and-surface-inventory) · [activities](#4-end-to-end-activities) ·
[verification](#5-existing-checks-and-future-verification) · [record](#6-coverage-findings-and-running-record) ·
[decisions](#7-decisions).

## 1. Purpose, scope and sources

Review diagnosis as a chain of evidence: notice a symptom, narrow its range and source, explain the failing work,
decide on a bounded response and verify recovery. Every chart or log link must retain enough context to continue
that investigation. Monitor is a section of System, not a separate launcher app.

Sources: [Monitoring guide](../docs/tools/Monitoring.md), [tools README](../runtime/components/base/tools/README.md),
[monitoring build record](archive/Monitoring.md), [diagnostic settings](archive/Monitor-Settings.md),
[archived review](archive/UI-Refinement.md), current `tools/screen/System/Monitor`, `tools/report/monitor-*.json`,
framework monitor/report services and `MonitoringSecurityData.xml`.

### Ownership and handoffs

| Surface | Owner / boundary |
| --- | --- |
| Monitor root and all 24 child definitions | This plan |
| Diagnostic settings and AI traces reached through Conf/Logging and Llm/Traces | This plan owns behavior; core System/AI plans inspect their wrapper entry and return |
| System Home/Health, job-run context | [System](ui-review-system.md); this plan owns the diagnosis after following the link |
| Shared Explore/report/Definition/Drill/LogContext/Split controls | [Dashboards](ui-review-dashboards.md) owns generic behavior; inspect Monitor's includes and supplied context here |
| AI runs, evaluations and provider setup | [AI/integrations](ui-review-system-ai-integrations.md); retain run/trace identity across the handoff |

No business-app monitoring data or real incident response is required. Recovery acts only on disposable local
fixtures and an explicitly prepared adapter; a manual instruction must not be reported as executed recovery.

## 2. Review method and refinement criteria

Apply the [common review protocol](ui-review-shared-gallery.md#common-review-protocol) and
[verification policy](ui-review-shared-gallery.md#verification-policy-for-later-execution). The details below add this
area's fixtures, dependencies and judgment criteria.

### Environment, identities and records

- Use `SYSTEM_APP` for Monitor access; there is no separate Monitor app group. Prepare `review.monitor.reader`
  from a view-only System grant and specialized holders of `MONITOR_REPLAY`, `MONITOR_INCIDENT`, `MONITOR_RECOVER`,
  `MONITOR_RECOVER_ADMIN`, `MONITOR_SETTINGS`, `MONITOR_INGEST` and `MONITOR_PRIVATE_NETWORK` as needed. Keep the
  missing-permission counterpart otherwise identical. The policy administrator and action approver are distinct.
  Both recovery operators also need `MONITOR_INCIDENT` to read Recovery/IncidentDetail; granting
  `MONITOR_RECOVER` or `MONITOR_RECOVER_ADMIN` alone does not make their starting page available. Record that
  read prerequisite separately from the action permission when pairing successful and refused operations.
- Seed local telemetry with related logs/spans/errors, a multi-service trace, resource metrics, one browser session,
  a synthetic business operation, two releases and a regression. Keep empty/disabled telemetry available as a separate
  startup/configuration case. Record source IDs, event times and trace links, not assumed “recent” demo dates.
- Prepare source-scoped ingestion keys, local synthetic endpoints, fake browser replay, a harmless recovery adapter
  with success/refusal/failure states, and a manual policy. Generated tokens are evidence-redacted and revoked after use.
- Network checks and ingestion operate only on local fixtures. A loopback target may require MONITOR_PRIVATE_NETWORK;
  a production URL is not a substitute. Temporary settings, policies, emergency-stop state and scheduled checks must
  be restored; do not change telemetry out from under another review without sequencing its consumers.

### Area-specific criteria

- Scope (instance/service/source), time range, timezone, trace/run/error identity and units survive every drill.
  Charts remain readable with real labels and zero/unknown values, not just sample placeholders.
- Diagnostic text keeps precision useful for short events; raw data stays inspectable without overwhelming the
  primary timeline. Replays keep masking and access controls in the displayed frames and downloaded events.
- Manual, proposed, approved, executing, refused, failed and completed recovery are distinct. An approval records
  whose action and what target it covers; no UI shortcut bypasses a current permission or policy check.
- Expiring settings show effective and on-disk values together. A request acknowledged or check skipped is not a
  fresh measurement, and a failed telemetry source must not blank unrelated evidence.

### Ordered waves

| Wave | Activities | Continuation |
| --- | --- | --- |
| 1. Diagnose a symptom | MON-01–05 | Overview → log/trace/error → metric comparison → service/operation |
| 2. Gather evidence | MON-06–08 | Browser replay, sources/keys, synthetic checks |
| 3. Respond and verify | MON-09, MON-11 | Release regression → incident → bounded recovery → repeat diagnosis |
| 4. Tune and reconcile | MON-10, MON-12 | Temporary settings/expiry and alternate mounts/modes |

Generate replay/check events before freezing the investigation range. Recovery uses a separate fixture from the
failure needed for comparison. Settings overrides run in a serialized batch and are reverted before baseline retests.

## 3. Screen and surface inventory

This inventory accounts for **25 XML definitions** assigned to this file; screen containers and helper
definitions are included. Routes below are Material paths unless labelled otherwise; app routes have Standard
`/apps` counterparts. A file's physical presence does not prove access or a supported interaction in every mode.
Each row maps to §4; helper inspection does not replace its parent workflow. Counts describe definitions, not user
activities. Dialogs, templates and non-XML surfaces follow the route tables.

### tools: `System`

| Route / surface | XML source | Default / variants / boundary | Activities |
| --- | --- | --- | --- |
| `/qapps/system/Monitor` | [Monitor.xml](../runtime/components/base/tools/screen/System/Monitor.xml) | Default: `Overview`. | MON-01, MON-12 |

### tools: `System/Monitor`

| Route / surface | XML source | Default / variants / boundary | Activities |
| --- | --- | --- | --- |
| `/qapps/system/Monitor/AgentTraces` | [AgentTraces.xml](../runtime/components/base/tools/screen/System/Monitor/AgentTraces.xml) | Context: `from`, `thru`, `provider`, `model`, `agent`, `errorsOnly`. | MON-12 |
| `/qapps/system/Monitor/Artifacts` | [Artifacts.xml](../runtime/components/base/tools/screen/System/Monitor/Artifacts.xml) | Includes `component://tools/screen/Dashboards/Dashboard.xml`. | MON-05 |
| `/qapps/system/Monitor/Browser` | [Browser.xml](../runtime/components/base/tools/screen/System/Monitor/Browser.xml) | Includes `component://tools/screen/Dashboards/Dashboard.xml`. | MON-06 |
| `/qapps/system/Monitor/Checks` | [Checks.xml](../runtime/components/base/tools/screen/System/Monitor/Checks.xml) | Inspect the route and its visible controls through the activity. | MON-08 |
| `/qapps/system/Monitor/ErrorDetail` | [ErrorDetail.xml](../runtime/components/base/tools/screen/System/Monitor/ErrorDetail.xml) | Context: `fingerprint`, `from`, `thru`. | MON-03 |
| `/qapps/system/Monitor/Errors` | [Errors.xml](../runtime/components/base/tools/screen/System/Monitor/Errors.xml) | Context: `from`, `thru`, `queryString`, `pageIndex`, `pageSize`, `order`. | MON-03 |
| `/qapps/system/Monitor/IncidentDetail` | [IncidentDetail.xml](../runtime/components/base/tools/screen/System/Monitor/IncidentDetail.xml) | Authored text: `qvt,html,html-print`, `tui`. Hidden/helper entry; enter through its parent action and inspect direct access. Context: `incidentId`. | MON-09, MON-11 |
| `/qapps/system/Monitor/Incidents` | [Incidents.xml](../runtime/components/base/tools/screen/System/Monitor/Incidents.xml) | Authored text: `qvt,html,html-print`, `tui`. Context: `statusId`, `serviceName`. | MON-09 |
| `/qapps/system/Monitor/LogContext` | [LogContext.xml](../runtime/components/base/tools/screen/System/Monitor/LogContext.xml) | Hidden/helper entry; enter through its parent action and inspect direct access. Includes `component://tools/screen/Dashboards/LogContext.xml`. | MON-02 |
| `/qapps/system/Monitor/Logs` | [Logs.xml](../runtime/components/base/tools/screen/System/Monitor/Logs.xml) | Includes `component://tools/screen/Dashboards/Explore.xml`. | MON-02 |
| `/qapps/system/Monitor/Metrics` | [Metrics.xml](../runtime/components/base/tools/screen/System/Monitor/Metrics.xml) | Includes `component://tools/screen/Dashboards/Explore.xml`. Context: `metricName`, `groupBy`, `metricStat`. | MON-04 |
| `/qapps/system/Monitor/Operations` | [Operations.xml](../runtime/components/base/tools/screen/System/Monitor/Operations.xml) | Includes `component://tools/screen/Dashboards/Dashboard.xml`. | MON-05 |
| `/qapps/system/Monitor/Overview` | [Overview.xml](../runtime/components/base/tools/screen/System/Monitor/Overview.xml) | Includes `component://tools/screen/Dashboards/Dashboard.xml`. | MON-01 |
| `/qapps/system/Monitor/Recovery` | [Recovery.xml](../runtime/components/base/tools/screen/System/Monitor/Recovery.xml) | Context: `editPolicyId`, `editAdapterId`. | MON-11 |
| `/qapps/system/Monitor/ReleaseDetail` | [ReleaseDetail.xml](../runtime/components/base/tools/screen/System/Monitor/ReleaseDetail.xml) | Authored text: `qvt,html,html-print`, `tui`. Hidden/helper entry; enter through its parent action and inspect direct access. Context: `serviceName`, `version`. | MON-09 |
| `/qapps/system/Monitor/Releases` | [Releases.xml](../runtime/components/base/tools/screen/System/Monitor/Releases.xml) | Context: `serviceName`. | MON-09 |
| `/qapps/system/Monitor/ReplayDetail` | [ReplayDetail.xml](../runtime/components/base/tools/screen/System/Monitor/ReplayDetail.xml) | Authored text: `html,html-print,text`, `qvt`, `tui`. Hidden/helper entry; enter through its parent action and inspect direct access. Context: `replayId`. | MON-06 |
| `/qapps/system/Monitor/Replays` | [Replays.xml](../runtime/components/base/tools/screen/System/Monitor/Replays.xml) | Authored text: `qvt,html,html-print`, `tui`. Context: `userId`, `visitId`. | MON-06 |
| `/qapps/system/Monitor/Services` | [Services.xml](../runtime/components/base/tools/screen/System/Monitor/Services.xml) | Includes `component://tools/screen/Dashboards/Dashboard.xml`. | MON-05 |
| `/qapps/system/Monitor/Settings` | [Settings.xml](../runtime/components/base/tools/screen/System/Monitor/Settings.xml) | Context: `changeKey`. | MON-10 |
| `/qapps/system/Monitor/Sources` | [Sources.xml](../runtime/components/base/tools/screen/System/Monitor/Sources.xml) | Inspect the route and its visible controls through the activity. | MON-07 |
| `/qapps/system/Monitor/Split` | [Split.xml](../runtime/components/base/tools/screen/System/Monitor/Split.xml) | Includes `component://tools/screen/Dashboards/Split.xml`. | MON-02, MON-04 |
| `/qapps/system/Monitor/TraceDetail` | [TraceDetail.xml](../runtime/components/base/tools/screen/System/Monitor/TraceDetail.xml) | Authored text: `html,html-print,text`, `qvt`, `tui`. Context: `traceId`, `from`, `thru`. | MON-02, MON-03 |
| `/qapps/system/Monitor/Traces` | [Traces.xml](../runtime/components/base/tools/screen/System/Monitor/Traces.xml) | Context: `from`, `thru`, `queryString`, `pageIndex`, `pageSize`, `order`, `spanKind`, `serviceName`, `spanName`, `statusCode`, `minDuration`, `maxDuration`, `userId`. | MON-02 |

### Embedded surfaces, dialogs, outputs and reconciliation

| Supporting surface | Activity / inspection |
| --- | --- |
| Overview/Services/Artifacts/Browser/Operations report embeds and panel menus | MON-01/05/06; inspect/export uses the owning transition, not a Material wrapper URL |
| Logs uses Explore, Metrics picker, LogContext and Split helpers | MON-02/04; shared report controls, Monitor-specific range/source/navigation |
| TraceDetail span timeline and raw/context disclosure; ErrorDetail grouping | MON-02/03; long labels, overlapping spans, precision and keyboard selection |
| Replay events fetch/player, masking, trace links and missing-event states | MON-06; a rendered ReplayDetail shell is not replay playback |
| Sources key create/revoke and one-time secret, counters/partition operation | MON-07; Sources is not a general database-maintenance screen |
| Checks create/edit/delete, Check/Scrape now, private-network consent | MON-08; distinguish stored definition, scheduled execution and latest result |
| Incident cause/evidence/notes/decisions/actions; Recovery policies/adapters/emergency stop | MON-09/11; use actual controls and result states |
| Monitor/Settings and Conf/Logging include; AgentTraces and Llm/Traces include | MON-10/12; wrappers listed in companion inventories and checked for context/access |

The archived review's replay/release/incident pages were partly inspected or rendered without complete interaction.
Their later fixes are history, not evidence for these workflows. No obsolete standalone Monitor launcher or old
Monitor-only artifact group is assumed.

## 4. End-to-end activities

Use visible controls and inspect intermediate states, confirmation and the resulting data/output. Unless narrowed
below, each activity has Material light/dark, actual supported TUI and a brief Standard/output branch. Select narrow
and keyboard checks where layout/interaction differs. Record each unattempted branch in §6; a service call or render
alone does not complete the activity. Names beginning `review.` are fixture proposals, not shipped accounts.

### MON-01 — Start from a symptom and retain its investigation range

- **Actor/start:** Monitor reader; known local error/latency symptom and a matching System attention item.
- **Objective/steps:** System Home/Health → Monitor/Overview → choose instance/range → inspect suspicious panel
  → follow relevant logs/metrics/traces → return; inspect/export embedded panel.
- **Expected:** Source, time range and units agree across entry and destination; panel actions keep the owning
  dashboard/transition; unavailable metrics are clear.
- **Recovery:** Telemetry off, no matching events, one failed panel and stale asynchronous result preserve
  independent diagnostics.
- **Screens/modes:** Monitor/Overview and embeds; System Home handoff, browser/TUI/Standard and narrow
  chart/header inspection.

### MON-02 — Follow a log to its trace and surrounding context

- **Actor/start:** Reader with a correlated multi-span trace and log entries before/after a selected event.
- **Objective/steps:** Logs → search/filter → choose row → LogContext → trace → TraceDetail → select span/raw
  data → related logs → Back; compare with Split.
- **Expected:** The same trace/event/range is retained; surrounding context explains ordering and precision; raw
  data is reachable by keyboard.
- **Recovery:** Malformed query, missing trace/span, expired retention, failed fetch and interrupted search keep
  editable input and explain absent evidence.
- **Screens/modes:** Logs, LogContext, Traces, TraceDetail, Split; server transitions and actual TUI links,
  readable long span labels.

### MON-03 — Explain an error group and its affected work

- **Actor/start:** Reader with repeated instances of one error and an unrelated error.
- **Objective/steps:** Errors → filter/count → ErrorDetail → inspect occurrence/context → TraceDetail/logs →
  related service/release → return to error group.
- **Expected:** Group, occurrence count, sample and selected trace are distinguishable; data supports the
  displayed explanation rather than inferred causality.
- **Recovery:** No occurrences in range, stale/deleted group, redacted detail and unavailable stack retain a
  usable return path.
- **Screens/modes:** Errors, ErrorDetail, TraceDetail, Services and release handoff; browser/TUI/Standard tables
  and keyboard disclosure.

### MON-04 — Compare a metric over meaningful ranges

- **Actor/start:** Reader with known metric units, service tags and a changed rate between two periods.
- **Objective/steps:** Metrics → select metric/group → change interval/range → inspect point → compare in Split
  → follow correlated context → inspect/export.
- **Expected:** Metric name, unit, aggregation and both windows stay visible; zero and no reading differ;
  abbreviated ticks retain useful full timestamp tooltips.
- **Recovery:** Unknown metric, empty comparison, incompatible grouping, delayed earlier query and missing
  export backend do not silently replace current data.
- **Screens/modes:** Metrics, Split, context links and embedded charts; browser visual/keyboard plus
  TUI/Standard numeric fallback.

### MON-05 — Move from service pressure to a specific operation

- **Actor/start:** Reader of service/artifact/operation telemetry; disposable service call recorded by Tools.
- **Objective/steps:** Services → select service → Traces/Errors → Artifacts → inspect relevant
  entity/screen/service → Operations → follow a linked execution → return to Tools detail.
- **Expected:** Summary and individual execution agree on identity/range; links retain the investigation context
  and respect destination access.
- **Recovery:** No instrumented operation, unavailable telemetry kind, missing Tools grant and failed summary do
  not masquerade as healthy zero work.
- **Screens/modes:** Services, Artifacts, Operations and Tools Service/Schema/Database embeds; scoped handoff,
  not a second service-run review.

### MON-06 — Replay a captured browser failure safely

- **Actor/start:** MONITOR_REPLAY holder and reader without it; synthetic browser session with masked sensitive
  fields and a trace.
- **Objective/steps:** Browser → Replays filter → ReplayDetail → load/play/seek → inspect event gap/error →
  follow trace → return to same session/time.
- **Expected:** Playback corresponds to the selected session; masking persists in events and frames; trace links
  relate to the captured action; visible controls do not imply absent events exist.
- **Recovery:** Missing/revoked replay, oversized/dropped capture, failed event fetch, no player support and
  changed permission keep private data unavailable and name the gap.
- **Screens/modes:** Browser, Replays, ReplayDetail/events and TraceDetail; browser playback, TUI/Standard
  truthful metadata/delegation.

### MON-07 — Admit a telemetry source and revoke its key

- **Actor/start:** MONITOR_INGEST holder with source-scoped permissions; local collector/fixture sender and
  reader.
- **Objective/steps:** Sources → create key for fixture source → capture token once securely → send a known
  local event → inspect source/counters → revoke → verify refused later send; inspect partition controls without
  using shared data.
- **Expected:** Source and key scope are clear; secret is shown once; accepted/dropped/export counters describe
  actual events and revocation is observable.
- **Recovery:** Invalid source, expired/revoked key, refused ingestion, unsupported partition backend and
  canceled action retain clear state without revealing tokens.
- **Screens/modes:** Sources and relevant log/metric destination; supporting local HTTP injection is fixture
  setup, visible source lifecycle is the reviewed activity.
- **Partition branch:** Inspect the Move telemetry tables into day partitions confirmation, its table/row counts,
  retained-old-table message and independent failures. Successful execution needs a separate disposable PostgreSQL
  fixture with an eligible unpartitioned table; otherwise record this as supporting inspection with execution
  blocked. Recheck the same telemetry query afterward, and keep the no-table-needed outcome distinct from a move.

### MON-08 — Run a synthetic check and follow its result

- **Actor/start:** Check operator with MONITOR_PRIVATE_NETWORK for local targets and a counterpart without it;
  harmless responsive and failing endpoints.
- **Objective/steps:** Checks → create/edit synthetic check and scrape target → Test/Run now → inspect last
  run/state → follow trace/metrics → disable/delete fixture.
- **Expected:** Target, schedule, last execution and requested run are distinct; private-network restriction is
  explained; each failed target is named independently.
- **Recovery:** Invalid address, timeout, no consent, disabled definition, skipped interval and canceled delete
  retain the previous valid result with its time.
- **Screens/modes:** Checks forms, trace/metric/dashboard links; browser/TUI/Standard, keyboard long
  configuration dialogs.

### MON-09 — Review a release regression and open an evidence-based incident

- **Actor/start:** MONITOR_INCIDENT holder and missing-permission reader; two synthetic releases with known
  change/error and one incident.
- **Objective/steps:** Releases → ReleaseDetail compare → inspect regression/new error → create/open Incidents →
  IncidentDetail → inspect evidence/cause → confirm/reject cause → add note/status → follow related traces.
- **Expected:** Comparison windows and release identities remain clear; suspected and confirmed cause differ;
  incident evidence links back to its source and changes retain actor context.
- **Recovery:** No comparable release, stale regression, missing evidence, refused status and canceled dismissal
  leave an honest unresolved record.
- **Screens/modes:** Releases, ReleaseDetail, Incidents, IncidentDetail; actual notes/decisions, TUI/Standard
  and narrow chronology.

### MON-10 — Apply an expiring diagnostic setting and restore the baseline

- **Actor/start:** MONITOR_SETTINGS holder and view-only counterpart; copied configuration and short-lived
  harmless override.
- **Objective/steps:** Monitor/Settings → inspect effective/on-disk values → change target/rule with expiry →
  visit affected diagnostics → open Conf/Logging → revert one/all or observe expiry → inspect baseline.
- **Expected:** Override scope, expiry and source are explicit; rejected values stay editable; reversion/expiry
  restores the intended value and both entry routes agree.
- **Recovery:** Invalid value/target, denied save, expired override, concurrent change and unavailable setting
  preserve the current baseline and report the exact result.
- **Screens/modes:** Monitor/Settings, Conf/Logging wrapper, System Conf/View; keyboard/narrow forms and actual
  TUI/Standard edits.

### MON-11 — Approve and verify a bounded recovery

- **Actor/start:** MONITOR_RECOVER_ADMIN policy administrator, MONITOR_RECOVER approver and reader; disposable
  safe adapter plus manual and failing outcomes.
- **Objective/steps:** Recovery → inspect/edit policy/adapter → incident recommendation → approve/refuse →
  inspect action state → complete/cancel/revert where offered → revisit original symptom; test emergency stop
  then restore.
- **Expected:** Policy, approval, execution and manual instruction remain distinct; target is named; adapter
  outcome and verified symptom determine completion.
- **Recovery:** Stale recommendation, lost permission, emergency stop, failed adapter, uncertain result and
  canceled confirmation do not trigger unintended retries or claim repair.
- **Screens/modes:** Recovery, IncidentDetail decision/action forms, diagnostics retest; local adapter only,
  TUI/Standard supported actions.

### MON-12 — Reconcile alternate mounts and restricted diagnostics

- **Actor/start:** Specialized holders, ordinary Monitor reader and outsider with earlier known-good records.
- **Objective/steps:** Open Llm/Traces (the AgentTraces include) → filter provider/model/agent/errors/time →
  TraceDetail → return → Usage and cost or AI dashboard; correlate the fixture's AI run where its links/IDs allow.
  Open Conf/Logging → settings →
  return; inspect remaining helper routes/output bytes and selected denial counterparts.
- **Expected:** Wrappers retain the same identity/range/permissions as their owned diagnostics; no access comes
  solely from hiding a tab or a supplied mount map.
- **Recovery:** Revoked permission, unsupported telemetry feature, missing target and failed include are
  explicitly unresolved rather than inferred passing coverage.
- **Screens/modes:** AgentTraces, aliases, helpers and every remaining inventory entry; cross-plan handoff with
  AI-03 and SYS-06.
- **Sampling branch:** Compare calls/tokens and the slowest-call duration with the known fixture spans; inspect
  the newest-call/trace limits and sampling/loss notices. These summaries cover the displayed traces, not a full
  billing ledger, and contain no prompts/replies. Missing telemetry is not proof that no provider call occurred.

## 5. Existing checks and future verification

This is an available-test map, not a results ledger. Read assertions and suite setup before selecting a run; static
shape checks, offline component fixtures, live browser walks and service rules provide different evidence.

| Component / suite | Existing specs | Coverage and limits |
| --- | --- | --- |
| tools / server | [MonitorChecks](../runtime/components/base/tools/test/MonitorChecks.spec.js), [MonitorSettings](../runtime/components/base/tools/test/MonitorSettings.spec.js) | Private-network refusal/holder and setting permission/override/revert rules; not the full UI activity. |
| tools / server | [AdminOverview](../runtime/components/base/tools/test/AdminOverview.spec.js), [SystemAttention](../runtime/components/base/tools/test/SystemAttention.spec.js) | Diagnostic date/readability/recovery form and attention boundaries; not causal investigation. |
| tools / acceptance client | [Monitoring](../runtime/components/base/tools/test/acceptance/client/Monitoring.spec.js) | Live logs-to-trace/context, metrics, check and panel controls; Browser/Replays/Releases/Incidents/Recovery portions include render-only checks. |
| webroot / client | [WebrootRum](../runtime/components/base/webroot/test/client/WebrootRum.spec.js), [WebrootReport](../runtime/components/base/webroot/test/client/WebrootReport.spec.js) | Offline telemetry/report client behavior, not capture/playback or full incident resolution. |

Future acceptance candidates: **MonitorDiagnosisWalk** (MON-01–05), **MonitorReplayWalk** (MON-06),
**MonitorSourcesChecksWalk** (MON-07–08), **MonitorIncidentRecoveryWalk** (MON-09/11) and **MonitorSettingsWalk**
(MON-10/12). Extend the existing Monitoring acceptance spec for overlapping paths. These all belong to the System
review's shared sweep ownership; do not add a separate Monitor regression sweep. Missing telemetry/adapter/media
fixtures remain blocked branches rather than reasons to weaken assertions.

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
| MON-01 — Start from a symptom and retain its investigation range | exercised — partial | Overview range/panel notices, populated error groups and System Health → Sources entry inspected. Known latency fixture, instance/range changes, chart drill/export, failed-panel recovery, dark/narrow/TUI/Standard remain pending. | MON-B01; MON-UI-002; SYS-UI-001 handoff |
| MON-02 — Follow a log to its trace and surrounding context | exercised — partial | Logs invalid-query recovery → real local log/context → trace selection/raw span → filtered logs, then invalid-duration/recovery in Traces. Trace/range retained. MON-B02 confirmed histogram total, partial-interval notice, zero-match and recovery. Split, controlled multi-service fixture, retention/missing-span/refusal/fetch-failure and alternate modes pending. | MON-B01/B02; MON-UI-001; evidence 02–08 |
| MON-03 — Explain an error group and its affected work | exercised — partial | MON-B02 opened a deliberately unnamed Overview group by its generated label and verified its full fingerprint, range and occurrences on ErrorDetail. Dedicated Errors, controlled repeated/unrelated errors, stack/release handoffs and recovery/mode branches pending. | MON-B01/B02; MON-UI-002; 01-overview |
| MON-04 — Compare a metric over meaningful ranges | exercised — partial | Charted a populated metric grouped by an attribute through the picker. Comparison of the same known series across windows and exported values remains open. | MON-B03; core-checks.txt |
| MON-05 — Move from service pressure to a specific operation | inspected — partial | Observed the populated Services and Instances aggregates in Material and opened Services in the live TUI. Remaining steps and recovery branches are open. | MON-B03 |
| MON-06 — Replay a captured browser failure safely | inspected — partial | Opened /qapps/system/Monitor/Replays as John Doe. Visible controls included Find, Collapse Replays. | MON-B03; entry-MON-06.json |
| MON-07 — Admit a telemetry source and revoke its key | exercised — partial | The browser created an ingest key and saw its single-display message. The fixture posted all three OTLP signals. Ingestion after revocation remains open. | MON-B03; core-checks.txt |
| MON-08 — Run a synthetic check and follow its result | exercised — partial | Created a Standard synthetic check and ran it from Material. Followed the passing result to a 3 ms trace and its 2.97 ms HTTP GET. Failed and recovered runs remain open. | MON-B03 |
| MON-09 — Review a release regression and open an evidence-based incident | inspected — partial | Opened /qapps/system/Monitor/Incidents as John Doe. Visible controls included Find, Collapse Matching incidents. | MON-B03; entry-MON-09.json |
| MON-10 — Apply an expiring diagnostic setting and restore the baseline | exercised — partial | MonitorSettings exercised a change and revert by the holder and a refusal by the reader. Expiry and consumer readback remain open. | MON-B03 |
| MON-11 — Approve and verify a bounded recovery | inspected — partial | Opened /qapps/system/Monitor/Recovery as John Doe. Visible controls included Collapse Adapters, Collapse Newest actions. | MON-B03; entry-MON-11.json |
| MON-12 — Reconcile alternate mounts and restricted diagnostics | inspected — partial | Opened /qapps/system/Monitor/AgentTraces as John Doe. Visible controls included Through, Search. | MON-B03; entry-MON-12.json |

### Findings register

Use `MON-UI-001` onward. P1 blocks work or essential understanding; P2 causes substantial friction or misleading
presentation; P3 is a localized refinement. Label evidence as observed behavior, static suspicion or design judgment.

| ID / severity | Actor, activity step, route, mode / viewport | Reproduction and impact | Evidence / owning layer | State / correction / retest |
| --- | --- | --- | --- | --- |
| MON-UI-001 / P2 | Observed; john.doe; MON-02; `/qapps/system/Monitor/Logs`; Material light/comfortable, 1920×1080 | On the fresh runtime, search `otel_logs` over `now-7d` → `now`. Documents over time says “No data in this time range” while the same page lists `1-20 / 2648`. Logs for a real trace similarly returned six rows with an empty chart. No partial-bucket or histogram-specific explanation is present in the full AX capture. The operator cannot distinguish an empty chart aggregation from absent log evidence. | `03-logs-recovered-message.txt`, `06-trace-logs.txt`. Candidate shared owner: [Explore.xml](../runtime/components/base/tools/screen/Dashboards/Explore.xml) lines 202–206 consumes histogram frames but does not append successful query notices; actual exclusion cause was not established. Generic report behavior hands off to [Dashboards](ui-review-dashboards.md). | Fixed and retested. Explore previously excluded edge buckets and discarded successful histogram notices. Its document histogram includes partial buckets, presents its own notices, and keeps loaded rows on a chart failure. In a fixed one-hour Logs range, Chrome’s panel inspector counted one edge event, one completed-bucket event, two events for their trace, and zero for an absent body. The partial and trace searches showed the partial-interval notice. Only the empty search displayed the no-data result. An isolated service fixture failed just the histogram. Logs showed the chart-specific error beside the two matching document rows. ExploreHistogram checks the single partial-bucket case against indexed data. **2026-11-25:** Passed the MON-B02 populated and empty searches. |
| MON-UI-002 / P3 | Observed in screenshot/AX; john.doe; MON-01/03; `/qapps/system/Monitor/Overview`; Material light/comfortable, 1920×1080 | In Top error groups, the third row has no visible Error text despite Type `error`, Service `moqui`, Occurrences `1` and Last seen. The underlying detail link has only its URL as its accessible name. The group cannot be identified by its visible error label. Reproduce with a group whose message is empty; observed fingerprint begins `5c58f00d`. | `01-overview.jpg` and `01-overview.txt`; [monitor-overview.json](../runtime/components/base/tools/report/monitor-overview.json) uses the message field as the detail link text without a visible fallback. | Fixed and retested. The Overview used a blank message as link text. Monitor now trims the message and type and falls back to an eight-character fingerprint label. Browser fixtures exercised all three cases. Opening Error group 9cee01ac reached the matching detail with its full fingerprint, from and thru parameters, matching heading and a positive occurrence count. **2026-11-25:** Passed the MON-B02 blank-caption drill. |

### Bounded batch record

For each later batch record:

- Intent, activities and branch limits; runtime/build/components, identities/grants, fixture IDs and dates.
- Observed steps and outcomes; evidence filenames, console/network/output observations and findings.
- Workarounds and blocked/deferred branches, with prerequisites and follow-up.
- In refinement only: decisions, owning files/components, correction, focused verification commands/results and retest.
- Temporary settings/grants/files/jobs restored, remaining cleanup and companion handoffs.

### MON-B01 — Initial diagnostic chain, 2026-11-16

**Scope and ownership.** First bounded review batch: System/Monitor Overview, Logs search recovery and surrounding
context, a real trace/span/raw-data/log round trip, Traces duration validation, and Sources as the System Health
handoff. The System review agent performed the browser walk; the monitoring reviewer inspected its saved screenshots
and full AX captures and owns this record. The monitoring child could not create an IAB tab, so its source preflight
alone did not advance coverage. No application fixes, key issuance, diagnostic setting changes, recovery actions,
service/job runs or automated tests were performed by this review. The full review pass remains open.

**Runtime and actor.** Current source `83cf05b3b6b069812a6ac2802d95f918db6a9802`; root prepared the current debug
binary after the pulled Rust changes, `target/debug/moqui`, reporting `moqui 0.1.0 (debug)` with telemetry and dev
features. SHA-256 `f799a82c681a7c3c9a7973cf5e030a5d1ebf41fa8935280f0d527dbf61bb2004`. Fresh disposable runtime
`/tmp/mr-ui-four-apps-zf5098dh`, embedded PostgreSQL, six individually linked components (framework, webroot, tools,
basic-comms, dev-aide, example), no addons. Copied `conf/UiReviewConf.xml`, explicit telemetry enabled and sample ratio
1 set before startup, scheduled jobs disabled. Full-sampling setup is a fixture prerequisite, not completion of
MON-10. Browser origin `http://127.0.0.1:18872`, distinct from root's `localhost` browser identity. Actor `john.doe`
(`EX_JOHN_DOE`, ADMIN + ADMIN_ADV); no least-privilege evidence. One shared runtime, lifecycle owned by root.

Live observations approximately 23:53–23:55 UTC / 17:53–17:55 America/Chicago on 2026-11-16, after startup at
23:49:20 UTC. Material light/default color scheme, comfortable density, 1920×1080. No dark/narrow/Standard/TUI or
complete keyboard/contrast walk. Telemetry consists of startup and local review requests, including a natural
new-device sign-in notification failure (`ACCOUNT_SIGN_IN` From address unset). No controlled incident, multi-service
causal fixture, known-rate comparison, captured-mail fixture or external provider was configured. Failure of that
automatic demo notice is context, not successful delivery or a new monitoring defect.

**Evidence.** Ignored local artifacts in `plans/ui-refinement-evidence/system-monitoring/batch-01/`:
`runtime-metadata.json`, `UiReviewConf.xml`, `preflight.md`, and files 01–10 below. Full AX captures provide actual
routes and values; screenshots were visually inspected. These are review evidence, not test baselines.

| Branch | Observed actions and result | Evidence / limit |
| --- | --- | --- |
| MON-01 Overview | Monitor defaulted to Overview with Last 7 days, refresh Off. HTTP request/duration panels explicitly reported exclusion of partial buckets and no matching completed bucket; their empty chart is not taken as zero traffic. Top error groups contained local sign-in/mail errors, Job runs showed a success row, and telemetry lost displayed 0. One error label was blank (MON-UI-002). | `01-overview.{jpg,txt}`. No range/instance change, point drill, export or independence-of-failed-panels check. Counts were current observations, not known expected fixture totals. |
| MON-02 Logs validation/recovery | Logs showed roughly 2,640 records initially. Query `(` returned `a group is missing ')' at position 1` and “The search could not run. Correct the query and search again.” Input and `now-7d`/`now` were retained, with results withheld. Clearing the query restored `1-20 / 2648` and Full message exposed the local sign-in notice failure. Documents over time remained unexplained empty beside the rows (MON-UI-001). | `02-logs-invalid.{jpg,txt}`, `03-logs-recovered-message.txt`. No query permission/refusal proof and no complete network/console trace. |
| MON-02 Context → trace → logs | Opened surrounding entries for document `001a1e6c8295698daac53c571`, anchor `2026-11-16T23:53:59.884Z`, then trace `c21330db041c4be8d6fe72d94c83ab09`. Selected `org.moqui.admin.IdentityServices.notify#NewDeviceSignIn`, ERROR, 12.30 ms, then expanded Raw span data. Waterfall also showed the failed EmailServices child. Logs for this trace returned six rows with `queryString=trace_id:c21330db041c4be8d6fe72d94c83ab09`; the range stayed `now-7d`/`now` throughout. | `04-log-context.{jpg,txt}`, `05-trace-selected.{jpg,txt}`, `06-trace-logs.txt`. Concrete trace continuity passed; this is one local request, not the planned multi-service fixture or complete causal investigation. Split and retention/fetch-failure branches pending. |
| MON-02 Traces validation/recovery | Monitor menu → Traces retained the trace query and range. Minimum duration `-1` remained visible with “Minimum duration is not a number of milliseconds and is ignored.” Results still showed the single POST request, 28.85 ms. Clearing and resubmitting removed the warning. | `07-traces-invalid-duration.{jpg,txt}`, `08-traces-recovered.txt`. Negative duration was explained as ignored; no boundary/invalid maximum or interrupted-query exercise. |
| MON-07 source inspection / System handoff | Sources showed queued 14,689, exported 14,686, dropped 0 and failed 0; the active `moqui` instance `1fc09a59-9b58-4949-8fa3-1054f6a6b96c` was recently seen at 23:54:51.798 UTC beside a stale startup/loading instance. System Health Fix reached this same Sources surface. | `09-sources.{jpg,txt}`, `10-health-fix-sources.txt`. Cumulative queued/exported difference is not asserted lost data. No ingestion/key mutation. System's misleading “Telemetry is off or not searchable” readiness message is owned once as [SYS-UI-001](ui-review-system.md#findings-register), not duplicated here. |

**Pending follow-ups.** Dedicated Errors/ErrorDetail, Metrics, Services, Operations, Artifacts, Split, all replay,
source admission/key, check, release/incident/recovery and diagnostic-setting workflows remain unexecuted. So do
restricted-user comparisons and alternate modes. Prepare controlled correlated errors and completed metric buckets
before asserting causal/rate/zero-versus-unknown outcomes. `preflight.md` separately records source-only candidates:
Last value aggregation semantics for a decreasing gauge, error-chart versus group-query scope, and the Overview
error link's first-seen→now range expansion. None is counted as a live finding or resolved activity.

**Cleanup and handoff.** No telemetry settings, grants, providers, keys or business records changed by this batch;
search URLs and local query history are disposable. Browser review finished and its viewport override was reset.
Root owns final shutdown of the shared runtime and its cleanup evidence; this batch did not stop it while the other
reviews were still using it. Keep the two findings open for consolidated triage after the review pass.


**Coordinated shutdown.** After the four browser batches, root stopped the shared server with SIGINT at
2026-11-17 00:02:38 UTC (Nov 16 locally), exit 0. Server/HTTP listener and runtime PostgreSQL processes/PID file
were absent; curl cookie file removed. Browser viewport overrides were reset. Session logout was not established;
server shutdown ended availability. Scratch runtime/database and ignored local evidence are retained. See
`runtime-cleanup.json` in this batch's evidence directory.

### MON-B02 — Post-cleanup recheck, 2026-11-25

The coordinator operated the built-in browser as `john.doe` in Material light at 1920×1080 on the disposable review runtime (release binary, checkout `6d8bb49f8a7c`).

- **MON-UI-001 passed.** On Logs, the `otel_logs` search from `now-7d` to `now` initially returned 1–19 of 3,026. On the next observation the chart inspector reported a final bucket of 3,130 and the list showed 1–19 of 3,130, with the partial-interval notice and partial flags on the interval edges. A nonmatching body search showed “No matching telemetry observations”, “No data in this time range” and “No results found” together. Clearing the query restored 1–19 of 3,136 with the partial-interval notice. The totals changed on this actively logging runtime.
- **MON-UI-002 passed.** A disposable fixture used the search facade to index and read back one `otel_errors` group with blank message/type, fingerprint `b0301125abcdef01`, service `review-monitor` and count 9,000 (fixture setup 1/1 passed). In Overview the coordinator clicked “Error group b0301125”. ErrorDetail retained the full fingerprint, `from=2026-11-25T11:35:45.39Z` and `thru=now`, repeated the label in the heading, and showed 9,000 occurrences. The seed is synthetic and remains confined to the disposable database.
- **System handoff.** While Sources showed 1,349 exported and 4,267 dropped alongside a recent and a stale publisher, System Health explicitly reported local exporter loss. [SYS-UI-001](ui-review-system.md#findings-register) owns the interpretation of these counters.

### MON-B03 — November 25 continuation

John Doe opened Metrics with 239 records. Services attributed 274 requests to a named instance with p95 latency 0.1847 s, and the heatmap separately displayed its empty state. A 110×34 live TUI opened Services. In Standard the operator created closeout-status for the isolated peer’s /status route. Material Run now reported a pass and trace 7a4509ee5f9aea9b0ff4f65b64611950. Evidence is in `monitor.json`, `service-pressure.json`, `synthetic-run.json` and `tui.txt`.

Following the synthetic result opened its trace with a 3 ms check, 2.97 ms HTTP GET and flame graph (synthetic-trace.json). The browser walks also sent local telemetry, applied filters, opened surrounding logs, charted grouped metrics and exported an overview panel (core-checks.txt). Replay and incident page renders establish entry only.

Local evidence is under `plans/ui-refinement-evidence/closeout-d10e22f/`.

Follow-up, 2026-11-25 (with DASH-UI-005): per the code and a probe spec render, Releases showed Released as raw UTC date text; it is now shown in the viewer's zone, with the zone named once, as ReleaseDetail shows it. Log context's times likewise. The regression test `monitor_tui::releases_show_times_in_the_viewer_s_zone` renders Releases once in the terminal and checks the Chicago text and "Times shown in America/Chicago.", and monitor_readers checks Log context's in html (it has no terminal template); the terminal sweep of every monitoring screen moved to acceptance (`cargo test-slow`). Not rechecked live.

#### Remaining branches

The ledger records the strongest observed outcome. The remaining steps, recovery paths and modes in each section 4 activity are deferred to the fixture and next check below. An explicitly blocked or failed result keeps that status.

| Activity | Required setup and next observation |
|---|---|
| MON-01 | Produce a timed symptom, carry the exact range to its related view and return. |
| MON-02 | Use a scoped diagnostic reader to follow a selected log into its trace and neighbouring rows. |
| MON-03 | Generate a recurring fixture error and verify its grouping by release and service. |
| MON-04 | Compare a known series across two time windows and verify its downloaded values. |
| MON-05 | Follow an individual slow operation from the service aggregate to its actual trace. |
| MON-06 | Enable sampling on a synthetic browser, produce a masked replay and check access as a second user. |
| MON-07 | Revoke a disposable source key and compare denied ingestion with an active key. |
| MON-08 | Make the check fail once, follow that trace, restore the endpoint and inspect recovered history. |
| MON-09 | Use two tagged releases and an induced regression to create and resolve an attributable incident. |
| MON-10 | Apply a short-lived diagnostic override and verify its consumer after expiry. |
| MON-11 | Use a disposable recovery adapter and separate approver to verify the target symptom before and after approval. |
| MON-12 | Compare alternate diagnostic mounts under a restricted reader and its grant holder. |

## 7. Decisions

- IDs are `MON`. This plan owns diagnostic behavior even when embedded elsewhere; app entry/context checks remain
  with their consuming plan.
- Synthetic fixtures establish causally related events and known outcomes; merely finding a recent log or drawing
  a chart is insufficient.
- Recovery remains bounded to local disposable targets. Unavailable actual replay or backend behavior is reported
  separately from a successful page render.

- Preparing or auditing these plans changes documentation only. Application/API changes, test runs, runtime walks
  and fixture creation belong to separate review/refinement execution.
- Current code, mounts and grants take precedence over older plans. Historical outcomes, permissions and exceptions
  are not inherited. Preserve routes, stored preferences and public contracts in any later refinement.
- Finish a review with explicit branch outcomes and a candid unresolved ledger; missing prerequisites do not become
  passes. Building absent features is outside this review.
