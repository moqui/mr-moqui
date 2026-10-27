# Monitoring

M1–M7 implementation results and completed acceptance checks are recorded below (Oct 26–27, 2026), including measured overhead, upgrade instructions and remaining gaps. **The remaining functional M1–M7 gaps are closed.** Final acceptance results, real-store/transport overhead and operational limits are recorded below. The original per-milestone overhead evidence remains incomplete because historical intermediate baselines were not captured. The later capabilities remain deferred. A new Monitor app in the tools component, built on OpenTelemetry, replaces ArtifactHit/ArtifactHitBin and the `moqui_logs`/`moqui_http_log` shipping, and takes in OTLP from external systems. Its charts, dashboards, alerts and scheduled reports come from a general reporting tool on the SearchFacade, with its own Dashboards app, that serves business reporting as well ([Dashboards and reports](#dashboards-and-reports)).

## Objective and scope

Provide a unified, technology-independent system that connects software changes, execution context, telemetry, source-level diagnostics, user and business impact, and recovery decisions.

Support backend services, browser, mobile and desktop applications, background workers, operating-system processes, containers, virtual machines, physical infrastructure, databases, networks, and AI-powered applications. Monitoring, diagnosis, and recovery must continue throughout each deployed version's lifetime, not only during releases.

## 1. Identify software, changes, and runtime exposure

- Record application/component identity, owner, team, source repository and revision, software version, immutable build artifact identity, build configuration, dependencies, and debugging symbols/source maps.
- Record every deployment, configuration, dependency, infrastructure, feature flag, AI model, and prompt change, including when it took effect.
- Track deployment environment, previous and candidate versions, rollout strategy, version overlap, and applicable recovery policy.
- Identify runtime instances and their lifecycle, host/device, operating system, infrastructure, executable version, and effective configuration and dependencies.
- Track which requests, users, sessions, and traffic populations encounter each version where measurable.
- Maintain authoritative identities and relationships so observations can be attributed to the exact software, deployment, runtime, and infrastructure without duplicating all metadata on every signal. Explicitly identify attribution uncertainty.

## 2. Collect and correlate telemetry

- Collect metrics, structured logs, distributed traces, profiles, stack traces, crash diagnostics, application/infrastructure events, real-user monitoring, session replay, business/product analytics, AI/LLM telemetry, network flows, network packet data, synthetic checks, security signals, audit records, change records, resource/topology data, and physical/device telemetry where applicable and supported.
- Capture relevant timestamps, identities, and context; normalize timestamps and account for clock differences.
- Preserve request, transaction, session, and causal relationships across clients, services, dependencies, networks, and infrastructure, including asynchronous tasks and message propagation where available.
- Support real-time analysis and historical investigation, including version-to-version comparisons.
- Validate schema compatibility, instrumentation coverage, and data quality; detect missing telemetry and broken correlation links.
- Apply sensitive-data filtering/masking, access controls, retention policies, and auditability throughout collection, investigation, and recovery.
- Support correlation of high-cardinality identities without requiring them as labels on every metric.

## 3. Capture failures and code-level performance

- Capture handled and unhandled exceptions, nested causes, hangs, deadlocks, resource exhaustion, crashes, and process termination reasons.
- Preserve available function/frame/module/source locations, asynchronous context, thread/task states, crash artifacts, and fault state. Retain symbols and source maps needed to resolve executable addresses to source code.
- Collect continuous or periodic CPU, memory, allocation, contention, and other supported profiles to attribute resource consumption to functions, threads, and execution paths.
- Link failures to profiles, logs, traces, active requests, dependency behavior, runtime state, and host/resource health before, during, and after the incident where possible.
- Connect frontend failures to application builds, devices, user sessions, and interaction history; connect backend failures to upstream callers and downstream dependencies.
- Use external diagnostics when a failing process cannot report, including abrupt termination, native crashes, out-of-memory kills, and host failures. Preserve partial evidence and explicitly show missing diagnostics.

## 4. Detect and quantify regressions

- Compare candidate versions against historical baselines, stable versions, or suitable control groups, using equivalent traffic, user populations, environments, and infrastructure where possible.
- Evaluate reliability, latency, resource consumption, security, user experience, business outcomes, and AI quality, latency, cost, and failure rates.
- Determine regression magnitude, duration, affected versions/instances/populations, severity, confidence, and data quality.
- Distinguish localized runtime/infrastructure faults from widespread version regressions, accounting for concurrent traffic, dependency, configuration, and infrastructure changes.
- Detect insufficient sample sizes and missing telemetry rather than interpreting them as evidence of health.
- Detect significant regressions without requiring a confirmed root cause first.

## 5. Diagnose probable root causes

- Identify the first observed deviation and preceding changes.
- Compare representative failing and healthy transactions, stacks, execution paths, logs, profiles, dependencies, resource usage, and source/configuration differences.
- Determine whether a fault originates in the affected component or propagates from a dependency, and identify affected user journeys and business transactions.
- Rank probable causes with supporting and contradicting evidence, relevant source revisions/functions/frames, timelines, and impact.
- Distinguish confirmed causes from correlation-based hypotheses and show unresolved uncertainty or missing evidence.
- Recommend relevant investigation or recovery actions.

## 6. Execute safe recovery

- Support continued observation, promotion, rollout pause, alerting, instance isolation, feature/configuration disablement, restart, rollback, and escalation for human approval.
- Select the least disruptive authorized action based on impact, confidence, recovery policy, permissions, sample volume, blast radius, and dependencies. Prefer targeted recovery for isolated faults.
- Permit automatic rollback only when sufficiently supported by evidence and verified as compatible with database schemas, persisted data, external interfaces, and dependent services.
- Escalate when no safe, authorized automatic action is available.
- Prevent repeated reversals, cascading actions, and conflicts between controllers.
- Preserve incident evidence and record the decision, rationale, policy, actor, authorization, execution history, and resulting deployment/runtime state.
- Execute recovery through existing deployment or operational controls.

## 7. Verify recovery and retain outcomes

- Verify that the intended action took effect and technical, user, and business indicators returned to acceptable behavior.
- Detect incomplete, failed, or harmful recovery; reopen or escalate unresolved incidents.
- Record the incident timeline, recovery outcome, resolution, corrective code changes, and confirmed root cause or unresolved status.
- Retain evidence for investigation, improvement, and compliance.
- Use incident findings to improve instrumentation, baselines, detection conditions, and recovery policies.

## 8. Provide a unified investigation experience

- Allow investigations to start from a version, regression, error, or incident rather than a telemetry storage type.
- Present severity, preceding changes, affected versions and infrastructure, technical/business impact, ranked causes, code-level evidence, distributed execution history, user experience, safe recovery controls, and recovery verification together.
- Allow direct navigation between errors, stacks, source code, profiles, traces, hosts, and deployment differences while preserving investigation context.
- Avoid requiring developers to manually combine evidence from independent monitoring systems.

## 9. Measure effectiveness

- Track detection time, cause-identification time, verified recovery time, regression detection precision/recall, automatic recovery success, false rollback rate, attribution coverage, symbolication coverage, profiling coverage, investigation effort, and business impact avoided.

---

## Operator UX: a 3 a.m. incident

- Usually no wake-up: authorized, reversible failures are contained or recovered automatically and verified against business health. Deduplicate/group alerts, suppress transient noise, and send a morning summary; page for critical integrity/security failures, unresolved business impact, failed recovery, or unsafe/unauthorized actions. Missing telemetry is not evidence of recovery.
- The page states what failed, severity, affected ERP operations/tenants, business impact, onset, current status, and recovery attempted/blocked, with one authenticated incident link and no sensitive document contents.
- That link opens one timeline: preceding build/config changes, affected versus healthy instances, baseline comparison, transaction outcomes, errors/stacks, granular JS hot functions/blocks, database/host evidence, ranked causes with uncertainty, and missing telemetry. Trace/profile/source links retain the incident context.
- Optional AI-assisted search: the operator asks “what changed, where is the JS bottleneck, and what is safe to do?” AI searches only authorized, tenant-scoped, redacted incident evidence and approved runbooks/source excerpts; answers cite evidence and flag uncertainty. Enforce filtering before indexing/model access, not just in prompts; exclude document contents, replay/raw dumps, credentials, arguments/locals, and secret-bearing source/config. Use pseudonymous references, audit searches/access, and never send raw ERP data to external models. AI has read-only scoped tools, no recovery authority; ordinary search and deterministic diagnosis remain available without AI.
- Show the next safe action, approval/compatibility checks, blast radius, and why automation stopped. The operator can pause/isolate/rollback through existing controls, then watch technical and business verification; never offer automatic reversal of committed business transactions.
- Faster than manually joining logs, dashboards, deployment history, and source: identity and causal links are already assembled, comparisons narrow the fault scope, and source-mapped evidence identifies where to investigate. Measure page-to-diagnosis and verified recovery time; do not promise instant fixes or hide uncertainty.

## Architecture

Decisions, Oct 25, 2026:
- **SDK:** use the OpenTelemetry SDK directly, with spans made from the artifact stack and no `tracing` bridge.
- **Storage:** SearchFacade indexes, on the Postgres or OpenSearch backend.
- **Ingest:** OTLP over HTTP first, with gRPC deferred.
- **Dashboards and reports:** a framework engine (datasets, report queries, frames, panels, dashboards, alerts) with its own Dashboards app in the tools component. Monitor is its first user. Business reporting, app landing pages and BOSS Insights use the same pieces.
- **Definitions:** dashboards, panels, datasets and rules are saved as JSON in entities, with versions and named references. Components ship them as JSON files, which are read-only until saved as a copy.
- **Order:** the report tool comes early (M2 and M3), so the Monitor screens are built from panels from the start.
- **References:**
  - SkyWalking (Apache-2.0) and SigNoz (MIT, apart from its proprietary `ee/` tree), for telemetry.
  - For dashboards and reports:
    - Grafana (AGPL-3.0, apart from the Apache-2.0 `packages/grafana-data`, `grafana-ui`, `grafana-runtime` and `kinds/` trees; its `apps/dashboard/kinds` schema is AGPL).
    - OpenSearch Dashboards (Apache-2.0; its reporting, alerting and observability plugins are separate repositories, not in the clone).
  - All are cloned under `~/work/moquiwork/3p_other/`. We borrow ideas from them, not code or schema text, and never read `ee/`.

### Starting point

- **Logging.** [`LineSubscriber`](../crates/moqui-host/src/logging.rs) is a hand-written `tracing` subscriber.
  - It hands out span ids but keeps no span data.
  - Events go to stderr, the TUI and SSH taps, and rolling files.
  - `ship_to_search` also sends them every 2 s to `moqui_logs`, and the `log_http` middleware ([`http.rs`](../crates/moqui-host/src/http.rs)) sends to `moqui_http_log`. Both go through `SearchFacade::index_log_entries`.
  - There is no `tracing-subscriber`, and no OpenTelemetry, prost or tonic crate in `Cargo.lock`.
- **Spans.** There are two `tracing` spans:
  - `artifact`, opened at each artifact-stack push and closed at pop ([`context/artifact.rs`](../crates/moqui-core/src/context/artifact.rs)). Screens, transitions, services, entities, REST and LLM calls all push frames.
  - `sql`, in [`entity/exec.rs`](../crates/moqui-core/src/entity/exec.rs).
  - There is no request span and no job span.
- **Hits.** `ExecutionContext::pop` calls `count_artifact_hit` ([`context/factory.rs`](../crates/moqui-core/src/context/factory.rs)).
  - [`context/hits.rs`](../crates/moqui-core/src/context/hits.rs) queues ArtifactHit rows and 900 s ArtifactHitBin bins, and writes them on the scheduler tick and at shutdown.
  - `clean#ArtifactData` removes old rows.
  - `stats-skip-condition` is configured but never read.
- **Metrics.** There is a plain-text `/metrics` endpoint (pool, request queue, statements, guards, replica lag), plus `status_map` and `thread_list` in [`context/status.rs`](../crates/moqui-core/src/context/status.rs).
- **Identity.**
  - Instance: `cluster::instance_id`.
  - Version: `CARGO_PKG_VERSION` and [`build_info.rs`](../crates/moqui-core/src/build_info.rs). There is no git revision.
- **Hand-offs.**
  - `CallJob` carries `user_name` and `origin` across the service pool ([`service/pool.rs`](../crates/moqui-core/src/service/pool.rs)).
  - Jobs write `ServiceJobRun` in [`service/jobs.rs`](../crates/moqui-core/src/service/jobs.rs).
  - Outbound calls go through [`outbound.rs`](../crates/moqui-core/src/outbound.rs).
- **SearchFacade.**
  - Aggregations are `Filter`, `DateHistogram`, `Terms`, `Sum`, `Extreme` and `CountDistinct` ([`search/agg.rs`](../crates/moqui-core/src/search/agg.rs)). Retention is `delete_matching`.
  - Gaps:
    - `DateHistogram` takes calendar units only (no `5m`), and fills gaps only between the first and last non-empty bucket.
    - `Terms` has no missing or other bucket.
    - Offset paging stops at `MAX_WINDOW` (10,000).
  - `aggregate` runs over one document type, and no service, REST endpoint or transition exposes it. Only mantle report services call it (`SalesReportServices`, `InventoryReportServices`). `search#DataDocuments` searches but takes no aggregations.
  - The Postgres backend keeps one table per document type in the `search` schema, with typed root columns and a `jsonb` body.
  - Business data is already indexed through DataFeeds:
    - `MantleSales` (`mantle_sales`): order parts and items, invoices and invoice items.
    - `MantleInventoryData` (`mantle_inventory`): inventory assets, order items, run consumption.
    - `MantleSearch` (`mantle`): parties, products, facilities and more.
- **UI.**
  - Chart.js 4.5.1 through `m-chart`, plus Mermaid, highlight.js through `m-code`, and `m-dynamic-container`.
  - Live data arrives through `/notws` topics (the `publish#ShellCounts` pattern) or by polling JSON transitions (the `Tools/Service/ServiceLoadRunner.xml` pattern).
  - Every qvt block needs a `tui` twin. The TUI already has `chart`, `timeline` and `tree` components.
  - `m-chart` takes a raw Chart.js config, and there is no XML chart element. mr-boss builds configs in `ironstone.projects.ReportServices.get#ChartConfig`, colours included. The TUI `chart` ([`chart.rs`](../crates/moqui-tui/src/components/chart.rs)) draws a table with one bar column.
- **Reports.**
  - `moqui.screen.ScreenScheduled` and `render#ScheduledScreen` ([`ScreenServices.xml`](../runtime/base-component/framework/service/org/moqui/impl/ScreenServices.xml)) render a screen with a saved find on a cron schedule. The output is csv, xlsx or PDF, emailed to a user or group or saved to a location, and is skipped when empty.
  - The PDF engine ([`pdf.rs`](../crates/moqui-host/src/pdf.rs)) is one headless Chromium. It loads each document into a blank page with every request failed. `Page.captureScreenshot` is already used by the test client's browser driver.
  - Form-lists have saved finds, aggregates and CSV/xlsx export.
  - BOSS Insights ([research](../runtime/component/mr-boss/plan/research-insights.md)) has a DataDocument report builder, saved and scheduled reports, and a placeholder Dashboards screen. There is no dashboard, widget or alert record yet.

### Shape

```
 business code ── moqui-core::telemetry (thin wrapper; no SDK types leak)
                        │  spans · metrics · logs
                  opentelemetry_sdk (simple span/log processors, periodic metric reader)
                   │                              │
          local exporter                  OTLP/HTTP exporter (optional)
                   │                              └──► external collector / backend
 OTLP/HTTP receiver ──► same documents
                   │
        SearchFacade indexes: otel_spans · otel_logs · otel_metrics · otel_errors · monitor_annotations
                   │              (and business documents, such as mantle_sales)
        report queries ──► frames ──► panels · dashboards · alerts · scheduled reports
                   │
        Monitor app and Dashboards app (tools component)
```

Internal and external sources share the same documents and the same screens, told apart by the `service.name` and `service.instance.id` resource attributes. Screens read the indexes only through report queries, which are authorized per dataset ([Datasets](#datasets)). The client side (the SDK and the exporters) replaces ArtifactHit/ArtifactHitBin and the `moqui_logs`/`moqui_http_log` shipping. The server side (the receiver) brings in telemetry from other systems.

### Crates

| Crate | Use |
|---|---|
| `opentelemetry` | API: tracer, meter, logger, context, W3C `traceparent` propagator |
| `opentelemetry_sdk` | Simple span/log processors feeding the bounded telemetry worker, periodic metric reader, logger provider and resource |
| `opentelemetry-proto` | OTLP messages (prost) for the receiver, and for our own exporter if needed |
| `opentelemetry-otlp` (http-proto) | Optional external exporter, used only if its HTTP client resolves to our reqwest 0.13. Otherwise a small exporter encodes with `opentelemetry-proto` and POSTs through `moqui_lib::http::RestClient` |

- **Versions.** All `opentelemetry*` crates go on one minor version, pinned by `Cargo.lock`.
- **Not used:** `tracing-subscriber`, `tracing-opentelemetry` and `opentelemetry-appender-tracing`. Spans come from the artifact stack and logs from `LineSubscriber`, so neither needs a `Registry`. tonic is deferred along with gRPC.
- **Check before adding.** Run `cargo tree` and look for:
  - tonic or a gRPC transport pulled in by the proto features
  - a second hyper or reqwest
  - a required async runtime (moqui-core has no direct tokio dependency)
- **Licenses.** These crates are Apache-2.0, which [`deny.toml`](../deny.toml) already allows. Add a row to [License-Review](archive/License-Review.md).
- **Wasm.** Keep them behind a `telemetry` feature that is off in `moqui-browser`.

### Signals

**Spans**
- **Request:** a SERVER root span per HTTP request, opened in `log_http`. It carries route, status, user, visitor and peer attributes, and replaces `RequestLog` and `moqui_http_log`.
- **Artifact:** a span per artifact frame, opened at push and ended at pop where the `artifact` span is today.
  - Attributes: `moqui.artifact.type`, `.name`, `.action`, `.sub_type`, user and visit ids, and the error status from `ec.message` errors.
  - These spans replace ArtifactHit rows.
  - User and visit ids are span attributes, never metric labels.
  - LLM call frames also carry the OTel `gen_ai.*` attributes: provider, model, input and output tokens, and finish reason. AI telemetry (requirement 2) then needs no separate store. An agent-traces view comes later.
- **SQL:** a CLIENT span per statement, with `db.system.name` and the parameterized `db.query.text`. The slow-statement warning and the statement counters stay.
- **Jobs:** a root span per `ServiceJobRun`, with the trace id saved on the run so the job screens can link to it.
- **Propagation.** W3C `traceparent`/`tracestate` is:
  - read on requests
  - carried in `CallJob` next to `origin`, so async services and pool work join the trace
  - injected by `outbound.rs` and remote services
  - written into the qvt page so `WebrootVue` fetches carry it (browser spans come later with RUM)
- **Sampling.** Head sampling is set in config. The local exporter keeps every trace that has an error or a slow root (a simple tail rule) and drops the rest of the unsampled traces.
  - Every request's root span is kept whatever the sampling, as `moqui_http_log` rows and screen ArtifactHits were written for every request. This keeps a visit's request history whole. Sampling and the tail rule apply to the spans below the root, and an unsampled root is marked so.
  - The tail rule holds each unsampled trace's spans in a bounded buffer until the root ends. The buffer caps the number of traces and the spans per trace; past the cap, the head decision stands.
  - A short-lived decision cache by trace id covers spans that end after their root, such as `CallJob` work.

**Metrics** (SDK instruments, with OTel semantic names where they exist)
- **Instruments:**
  - `http.server.request.duration`
  - `moqui.artifact.duration`, labelled by type and name (and action where there is one). It replaces ArtifactHitBin: artifact names are a bounded set, and the bin becomes one minute.
  - `db.client.operation.duration`
  - `moqui.job.runs`, counted by outcome
  - observable gauges for pool, request queue, guards and process, from the values `/metrics` and `status_map` already compute
- **Export:** the periodic reader runs every 60 s and writes one document per series per interval (count, sum, min, max and histogram buckets). This is SkyWalking's pre-aggregation done by the SDK, so RED charts never scan raw spans.
  - The reader uses delta temporality, so each document holds one interval's values, and a series with no activity in an interval writes nothing.
  - Histogram boundaries are fixed per instrument (the semantic-convention boundaries for durations), so documents of one series can be merged.
- **Volume:** there is at most one minute document per active series, which is series × instances × 1440 a day. For example, 2,000 artifact series on one instance give about 2.9 M documents a day. M2 measures this on Postgres. Each instrument has an attribute allow-list, so the series stay bounded.
- **Rollups:** a job rolls minute documents up to hour and day documents.
- **`/metrics`** stays, and can also serve Prometheus text from the same readers.

**Logs**
- `LineSubscriber` keeps stderr, the files and the taps.
- Instead of `ship_to_search`, it sends each event to an OTel `Logger`, taking trace and span ids from the current artifact frame, OTel severity numbers, and its event fields as attributes.
- DEBUG and TRACE are still not exported by default, and the exporter's own thread is skipped.

### Indexes

`search/telemetry.rs` generalizes [`search/logs.rs`](../crates/moqui-core/src/search/logs.rs): flat mappings that are not DataDocuments and work on both backends. Field names follow OTel (and SigNoz): `trace_id`, `span_id`, `duration_nano`, `severity_number`, `resource.*`, `attributes.*`.

| Index | Key fields (typed columns; the rest stays in the body) |
|---|---|
| `otel_spans` | trace_id, span_id, parent_span_id, name, kind, start, start_nano, duration_nano, status_code, service.name, service.instance.id, moqui.artifact.type/name, user and visit ids, attributes, events |
| `otel_logs` | timestamp, severity_number/text, body, trace_id, span_id, service.name, instance, attributes |
| `otel_metrics` | metric name, series fingerprint, bucket start, granularity (minute/hour/day), count, sum, min, max, buckets, attributes |
| `otel_errors` | fingerprint (hash of type, normalized message and top frames), first_seen, last_seen, count, sample trace ids, service, last message/stack |
| `monitor_annotations` | time, time_end, source (deploy, config, release, alert, user), tags, text, scope (service, instance, dashboard, panel), created_by |

- **Time.** OpenSearch `date` holds milliseconds, and the facade has no nanosecond field kind. So spans keep `start` for ranges and histograms, and `start_nano` (a long) for ordering and the waterfall.
- **Errors.** The local exporter upserts `otel_errors` when a span ends in error or an exception event is recorded. This is the SigNoz groupID idea.
  - The count is an atomic increment, so instances do not overwrite each other: `ON CONFLICT … DO UPDATE` on Postgres, and a scripted upsert on OpenSearch. This is a new facade capability.
- **Annotations** are requirement 1's change records. They hold:
  - deploy, config and release markers
  - alert state changes
  - notes added by hand on a chart

  CI posts deploy markers to a REST endpoint in `moqui.rest.xml`, authorized like an ingest key. Time series panels draw annotations as markers and bands.
- **Retention.** A daily ServiceJob replaces `clean#ArtifactData`. It deletes by time with `delete_matching`, with separate settings per index and per metric granularity: raw spans and logs for days, minute metrics for days, hour and day metrics for months, and annotations for years.
- **Later:** a Postgres-backend option to partition these tables by day, so retention drops partitions instead of deleting rows (SkyWalking's day tables).
- **Telemetry config.** It replaces `artifact-stats`. The settings: per-type enable and sampling (keep the artifact-type list), retention, the exporters, and the receiver. `stats-skip-condition` either becomes a sampling rule or goes away. An `artifact-stats` left in a site or component conf still applies, with a deprecation warning ([Retired and replaced](#retired-and-replaced)).

**SearchFacade additions** for the telemetry screens:
- `Avg` (or sum and count read together)
- `Percentiles`: `percentile_cont` on Postgres, native on OpenSearch
- a histogram-bucket merge for metric documents, so p50/p95/p99 can come from the SDK buckets without reading raw spans. It merges only series with the same boundaries.

These go into `Agg` and both backends, with tests on each. The report tool needs more; see [Report queries](#report-queries).

### Ingest

- **Endpoints:** `POST /v1/traces`, `/v1/logs` and `/v1/metrics` on the existing axum server, taking `application/x-protobuf` and `application/json`. Responses follow OTLP, including partial success.
- **Auth:** an API key tied to a user with a `MONITOR_INGEST` permission. Each key names the source, and the source is recorded on what it sends.
- **Limits:** body size, rate per key, and a bounded queue that rejects when full and counts what it rejects. Redaction rules run before indexing.
- **Decoding:** `opentelemetry-proto` decodes into the same documents the local exporter writes, so every screen works on external data unchanged.
- **Temporality:** many senders, Prometheus-style ones among them, send cumulative points. These become deltas through a bounded per-series cache of the last point, keyed by series fingerprint and start time; a lower value or a new start time is a reset. Exponential histograms are stored as sent, and merging them comes later.
- **gRPC** (tonic on 4317) is deferred behind a feature.
- **Pulling later:** a ServiceJob that scrapes Prometheus-text endpoints into `otel_metrics`, and synthetic checks recorded as spans.

### Failure tolerance

- Telemetry never blocks or fails business work. Exports are bounded, run off the request thread, and never write inside a business transaction.
- A failed batch, a document that will not index, or a bad OTLP record is logged once (rate-limited), counted, and skipped.
- When the search backend is down, telemetry is dropped and loss counters go up. Those counters show on `/metrics` and on the Monitor Sources screen.
- Missing telemetry shows as missing, never as healthy (requirement 2).

### Monitor app

`runtime/base-component/tools/screen/Monitor.xml` is added to `apps.xml` from `tools/MoquiConf.xml` at menu-index 97 (System is 98, Tools is 99). It uses icon `sym_o_monitoring` and a `MONITOR_APP` artifact group in `ToolsSecurityData.xml`.

Most of its screens are dashboards shipped as files, or Explore presets from the [Dashboards app](#placement). Traces, Errors and Sources stay specialized screens.

| Screen | Built as | Content | Replaces |
|---|---|---|---|
| dashboard | Dashboard | RED per service, top error groups, slowest artifacts, loss counters | |
| Services | Dashboard | Service, then endpoints/artifacts, then instances (a `service` variable, drill-down by data links). Rate, errors and duration percentiles over time | |
| Traces | Screen | Search by service, name, status, duration, user and time, then TraceDetail (waterfall, span attributes, logs for the trace) | |
| Logs | Explore preset | Query, filter, context and live tail over `otel_logs`. Each row links to its trace and span | `System/LogViewer.xml` |
| Errors | Screen | Fingerprint groups, then detail (count over time, samples, stack in `m-code`, traces) | |
| Metrics | Explore preset | Pick a metric, filter by attributes, group by, chart, and add to a dashboard | |
| Artifacts | Dashboard | Artifact duration and count by type and name, from `moqui.artifact.duration` | `ArtifactHitBins`, `ArtifactHitSummary` |
| Sources | Screen | Ingest keys, the latest data from each source, exporter status, drops | |
| Later | | Releases (version comparison, M6), Incidents (requirements 4–7) | |

**Alerts** are the Dashboards app's rules (M6), over `otel_metrics` or any other dataset ([Alerts](#alerts)). They replace SkyWalking's rule shape. Alertmanager is not used.

**JS and UI**
- **Reuse, with no new library to start:**
  - Chart.js for every chart, through `m-report-panel`, which builds on `m-chart`
  - `m-code` for stacks
  - Mermaid for a simple service map
  - `/notws` topics for the log tail
  - polling JSON transitions for dashboard refresh
- **New in-house components:**
  - `m-trace-waterfall`, plain positioned divs with no library
  - `m-report-panel` and `m-dashboard` ([Panels](#panels))
- **TUI twins:** the `timeline` component (spans and logs), `tree` (span tree), and `chart`, grown into the panel twins.
- **Library candidates only if needed** (each added through `libs/refresh.mjs` and License-Review):
  - uPlot (MIT, as SigNoz uses) if Chart.js is too slow for dense series
  - gridstack.js (MIT) for dragging and resizing panels in edit mode, if an in-house grid is not enough
  - an in-house flame graph when profiles arrive
  - `rrweb` for opt-in replay, as below

### Retired and replaced

Decided Oct 25, 2026. Three old tools go:
- the ArtifactHit and ArtifactHitBin entities and their System screens
- the System Log Viewer
- the System DataDocument Search

Each is replaced in the same milestone, so no function or data is lost in between. Visit and Visitor stay: they are business data, and visit ids link to spans.

**ArtifactHit and ArtifactHitBin (M2)**
- **Removed:**
  - Entities: ArtifactHit, ArtifactHitBin and the ArtifactHitReport view (`ServerEntities.xml`), their fence rows ([`entity/fence.rs`](../crates/moqui-core/src/entity/fence.rs)), and the nontransactional-group note in `MoquiDefaultConf.xml`.
  - Recording: [`context/hits.rs`](../crates/moqui-core/src/context/hits.rs), `count_artifact_hit`, and the bin flush at shutdown in `moqui-host`.
  - Screens and REST: the System `ArtifactHitSummary.xml` and `ArtifactHitBins.xml` screens, and their REST list entries in `tools/service/moqui.rest.xml`.
  - Cleanup: `clean#ArtifactData` and its `clean_ArtifactData_daily` job, replaced by the telemetry retention job.
- **`artifact-stats`.** The defaults move to the telemetry config. An `artifact-stats` left in a site or component conf still applies, with a deprecation warning, the way schema problems stay warnings: `persist-bin` turns the type's metric on or off, and `persist-hit` its spans.
- **`track-artifact-hit="false"`** stays on the screens that set it (`webroot` `sso.xml`, `emailOAuth.xml`, `echopath.xml`, `rest.xml`), as does the per-frame flag in `screen/render.rs`. It now means no artifact spans or artifact metrics under that screen. The request span is still written.
- **Slow frames.** The slow rule moves into the telemetry module, reusing the per-artifact running mean and variance in `hits.rs`: a frame is slow at more than 2.6 standard deviations above its artifact's average, after 50 frames.
  - A slow frame gets `moqui.artifact.slow` on its span and counts in a `moqui.artifact.slow` counter.
  - The tail rule keeps the trace of any slow frame over 1 s, as a slow hit over 1 s was always written.
- **Old rows are not migrated.** The tables stay in existing databases unused, including databases moved from Moqui Java, and the upgrade notes give the drop statements.

| Old | What it does | Replacement |
|---|---|---|
| System Artifact Summary (`ArtifactHitSummary.xml`, over `ArtifactHitReport`) | Per type, sub type and name: hits, total, min, max, avg, std dev, slow hits and last hit. Filter by type, sub type and name; sort on any column; saved finds and column choice; CSV, xlsx, text, XML and PDF | The Monitor **Artifacts** dashboard. A table panel over `moqui.artifact.duration` and the slow counter, with type, sub type and name variables, over any time range (rollups keep long ranges cheap). Columns: hits, total, min, max, avg, p50/p95/p99 from the buckets, std dev estimated from the buckets, slow hits, and the last active minute. Column sort, CSV and xlsx in M2; saved dashboards in M3; PDF through reports in M7 |
| System Artifact Bins (`ArtifactHitBins.xml`) | The same stats per 15-minute bin, filtered by bin start; CSV, XML and PDF | The same dashboard: a time series, and the table split by a date histogram (minute, hour or day, by range) from the minute documents and their rollups |
| Visit detail (`Visit/VisitDetail.xml`) | Every screen and transition hit of a visit: time, user, type, name, parameters, running time, slow, size, error and message, request and referrer URL, server | The visit's request spans, found by `moqui.visit.id`, each opening its trace. Each request span also carries the screen or transition it ran (`moqui.artifact.type` and `.name`) and its error, so the list stays whole when the spans below it were sampled out. The other columns come from the span attributes in the HTTP log mapping below. Parameters are the redacted `url.query`; service parameters are never recorded |
| Visit list (`Visit/VisitList.xml`) | Hit count per visit | A terms count of request spans over the visit ids on the page, one query per page |
| `example` Gallery `Report.xml`, and its TUI snapshot `gallery-Report-180x50.txt` | The report archetype over ArtifactHitBin | The same archetype over a report query on `otel_metrics`. It keeps its inline sample for an empty store, and the snapshot is updated |
| Gallery `Record360.xml` | A user's hit count and recent hits | Request spans by `user.id` |
| `Tools/Entity/SpeedTest.xml` | Times a render of Artifact Summary | Times a render of the Artifacts dashboard |
| `DataSnapshot.xml` comment; the openui skill (`openui-lang.md`, `llm/ui/openui_tests.rs`); the Assist agent spec and two replays; `WebrootAssistCanvas.test.js` | Use ArtifactHitSummary as an example path | Use a Monitor screen |
| System dashboard Usage box | Links to Artifact Hit Summary, Artifact Hit Bins and Log Viewer | Links to Monitor's Artifacts and Logs |

**Log Viewer (M2)**
- **Removed:** `System/LogViewer.xml`, `ship_to_search`, and the `moqui_logs` and `moqui_http_log` mappings in [`search/logs.rs`](../crates/moqui-core/src/search/logs.rs).
- **The Monitor Logs screen** is Explore on the `otel_logs` dataset. It covers each Log Viewer function:
  - The free `indexName` box becomes the dataset picker, which covers logs, spans (the old HTTP log) and documents.
  - The query string is the same Lucene subset, with a syntax link. The full query is shown, and a query that does not parse shows its error.
  - A time range is always set, the last 7 days by default.
  - Paging with a page size. Newest first, or oldest first within a page for reading like a tail, which is the Log Viewer's order.
  - Columns:
    - time to the millisecond
    - thread name and id
    - level, coloured
    - host
    - user, linked to the user account
    - visitor
    - logger
    - message, with long messages folded into a dialog
    - the exception, with type, message and the stack in `m-code`
- **New in the Logs screen:** links to the trace and span, the lines around an entry, a live tail, filters from a value, and CSV export.
- **`moqui_logs` fields map to OTel log records:**

| Old field | OTel |
|---|---|
| `@timestamp` | timestamp |
| `level` | severity text and number |
| `thread_name`, `thread_id` | `thread.name`, `thread.id` (`thread_priority` is dropped) |
| `source_host` | resource `host.name` |
| `user_id` | `user.id` |
| `visitor_id` | `moqui.visitor.id` |
| `logger_name` | scope name |
| `message` | body |
| `thrown` | `exception.type`, `exception.message`, `exception.stacktrace` |
| `mdc` | attributes |

- **`moqui_http_log` fields map to request span attributes:**

| Old field | OTel |
|---|---|
| `remote_ip` | `client.address` |
| `remote_user` | `user.id` |
| `session` | `session.id` |
| `server_ip`, `request_host` | `server.address` |
| `request_method` | `http.request.method` |
| `request_scheme` | `url.scheme` |
| `request_path`, `request_query` | `url.path`, `url.query` (redacted) |
| `http_version` | `network.protocol.version` |
| `response` | `http.response.status_code` |
| `time_final_ms` | the span's duration |
| `time_initial_ms` | `moqui.http.time_to_first_byte` |
| `bytes` | `http.response.body.size` |
| `referrer` | `http.request.header.referer` |
| `agent` | `user_agent.original` |
| `content_type` | `http.response.header.content-type` |
| `visitor_id` | `moqui.visitor.id` |

- **Both backends.** The comments in `logs.rs` and `LogViewer.xml` say log indexes exist only on OpenSearch. That is stale: Postgres keeps them too (`keeps_log_indexes`, `crates/moqui-host/tests/search_logs_postgres.rs`). `otel_logs` is on both backends as well.

**DataDocument Search (M3)**
- **Removed:** `System/DataDocument/Search.xml`. The System dashboard's Data Documents box links to Explore in its place, and Edit, Export and Feed Index stay.
- **Explore on DataDocument datasets** covers each of its functions:
  - choosing an index, from the distinct `DataDocument.indexName` values, as an index-wide dataset
  - a query string
  - results across the index's document types, each with its type name and id and the expanded `documentTitle`
  - the `DataDocumentLink` links, with their conditions
  - a View Document dialog with the full document
- **It adds:** highlights, and the retrieval mode (lexical, semantic or hybrid) where the backend has them. It also adds filters, field values, a count over time, and export.
- **`search#DataDocuments`** stays as a service and REST endpoint for other callers.

## Dashboards and reports

Planned Oct 25, 2026. This is a general reporting and charting tool on the SearchFacade, after OpenSearch Dashboards (from Kibana) and Grafana. The same pieces serve system telemetry and business reporting, such as sales by month, orders by status or receivables aging. Monitor is the tool's first user, not its owner.

The pipeline is Grafana's panel model, with OSD's aggregation editor as the query: dataset, then report query, then frame, then transformations, then panel. Panels go on dashboards. Alert rules reuse their queries, and scheduled reports reuse the dashboards.

### Placement

- **Engine:** a `report` module in moqui-core, beside `search`. It holds datasets, report queries, frames, transformations and alert evaluation.
- **Widgets** in webroot, each with a `tui` twin:
  - `m-report-panel`, one panel
  - `m-dashboard`, a layout of panels
  - an XML element that places a saved panel or dashboard on any screen, such as an app landing page or a record page
- **Dashboards app:** `runtime/base-component/tools/screen/Dashboards.xml`, at menu-index 96 with icon `sym_o_dashboard` and a `DASHBOARDS_APP` artifact group.

| Screen | Content |
|---|---|
| dashboard | Starred and recent dashboards, then all of them by folder and tag |
| Dashboard | View and edit one dashboard, its versions, kiosk mode and playlists |
| Explore | Ad hoc queries over a dataset, with split view |
| Panels | Library panels, and where each is used |
| Datasets | Datasets, their fields and field defaults |
| Alerts | Rules, firing instances, state history, silences, mute timings, contact points |
| Reports | Scheduled dashboard reports, with preview and send now |
| Annotations | Change markers by source and tag, plus manual notes |

- **Users of the tool:**
  - **Monitor** ships its dashboards as files, and links into Explore with the telemetry datasets.
  - **Business components** ship dashboards on their own datasets, such as mr-mantle on `mantle_sales`.
  - **BOSS Insights** reuses the screens and widgets under its own menus. This answers the open questions in its [research](../runtime/component/mr-boss/plan/research-insights.md#17-gaps--boss-implications):
    - Library panels are the one widget registry for dashboards and landing pages.
    - The renderers are framework code.
    - The report builder is a dataset plus a report query, with grain, pivot and period comparison.
    - Alerts are rules on the same query a dashboard panel shows.

### Datasets

A dataset is what OSD calls an index pattern: what a query may read, and how its fields display. A dataset has:
- **A document type and its time field.** The type is a DataDocument, or a flat telemetry mapping such as `otel_spans`.
  - An index-wide dataset covers every type of a DataDocument index, for general document search. It uses `_document` as a field and `_timestamp` as its time field.
- **Fields,** with kind, aggregatable and searchable, read from the mapping. A DataDocument dataset takes them from `DataDocumentField`.
- **Field defaults:**
  - unit, decimals and format
  - an entity lookup, which shows an id key as its entity's description, read in batches through the entity cache
  - value mappings from StatusItem and Enumeration
- **A signal type** (traces, logs, metrics or business), which picks Explore's default views.
- **Read permission:** artifact authorization on the dataset, plus `DataDocumentUserGroup` for a DataDocument.
- **Row filters** for the viewer (for example, by organization). The server adds them to every query; they are never taken from the client.

### Report queries

A report query names a dataset, a time range, a query string, filter pills, and an ordered list of aggregations, each `{id, type, role, params}`.
- **Roles** are metric, x-axis, split series and split rows or chart (OSD's schemas). Each visualization sets how many of each role it takes.
- **Compiling.** The list compiles to the nested `Agg` tree: bucket aggregations nest in order, with metrics at the leaves.
- **Tabifying.** The response is flattened into a frame: a column per bucket and per metric, a row per leaf bucket, and subtotals at each level when asked for.
- **Time:**
  - Relative ranges use Grafana's syntax (`now-6h`, `now/d`, `now-1M/M`, `now/w`, `now/fy`). They are parsed in Rust, with the week start and fiscal year start from settings.
  - The auto interval aims at a target bar count, capped by a maximum and rounded to a fixed table (OSD's approach). It says when it has scaled a requested interval up.
  - The bucket still filling is dropped or drawn as partial, so a rate chart does not show a false fall.
- **Filters:**
  - Pills can be pinned, negated, disabled, edited as a query, and labelled.
  - Clicking a bar, slice or cell turns its bucket into a pill: Other becomes a negated list, and Missing becomes "does not exist".
  - Brushing a time chart sets the range.
- **Service source.** A service can return rows or a frame instead, for reports the facade cannot compute, such as the `LedgerReportServices` statements or budget against actual. A panel can mix queries of both kinds.

**SearchFacade additions** for the report tool. They come with Avg, Percentiles and the bucket merge from [Indexes](#indexes), go into `Agg` and both backends, and have tests on each:
- **DateHistogram:** fixed intervals (`10s`, `5m`, `7d`), extended bounds so the whole range is filled, and dropping partial buckets.
- **Terms:** Missing and Other buckets, and `min_doc_count`. Postgres computes Other in the same SQL; OpenSearch uses a second filters request, as OSD does.
- **Metrics:** ValueCount, Stats, Median, and TopHits. TopHits gives sample documents per bucket, such as the trace ids behind a latency bar.
- **Buckets:** Histogram, Range and DateRange (for aging buckets), and named Filters.
- **Across types:** counts, terms and date histograms over all the types of an index, run per type and summed, for index-wide datasets. Today `aggregate` takes one type, apart from terms on `_document`.
- **Cursor paging** (`search_after` on time plus a tiebreaker), for log context and for exports past `MAX_WINDOW`.
- **Counter upsert:** the atomic upsert that `otel_errors` needs.
- **Guards:** a timeout and a bucket cap per query, with a "too many groups" notice in place of a failure.
- **A generic service:** an authorized service and REST transition that runs a report query. Today `aggregate` has no service at all.

Pipeline aggregations are not pushed to the backends. Derivative, cumulative sum, moving average, and ratios such as error rate are frame transformations instead.

### Frames and transformations

- **Frame:** one columnar shape for tables and time series.
  - Each field has a name, a type (time, number, string, boolean), a unit, labels and values.
  - Long and wide forms convert to each other.
  - The same frame feeds Chart.js, tables, the TUI, CSV and xlsx, alerts, scheduled reports and Assist tools.
- **Transformations** run in Rust on the server, so the browser, TUI, PDF, alerts and API all get the same numbers. Each step can be turned off, and the inspector shows each step's input and output. The first set:
  - reduce, from a series to one value per calculation: last, min, max, mean, sum, count, delta, percentiles
  - join by time, merge, group by
  - organize: rename, hide, reorder
  - calculate a field: binary math, running total, moving average, delta, ratio, percent of total
  - filter by value, sort, limit, partition by values
  - pivot (row, column, cell), with row and column totals
  - time series to table, with a sparkline column
  - time shift, and compare to the previous period
- **Totals** are computed from the data, not from the displayed rows (OSD averages the rows it shows).

### Panels

A panel is queries, then transformations, then a visualization with its options and field config.

- **Visualizations, first set:**
  - time series: line, area and bars, with stacking and a second axis
  - bar chart over categories
  - stat: a value with a sparkline, and percent change against the previous period
  - gauge and bar gauge
  - table: footer totals, frozen columns, and sparkline, colour and link cells
  - pie and donut
  - heatmap: latency from histogram buckets
  - state timeline
  - logs
  - the trace waterfall
  - text: Markdown with variables
- **Visualizations, later:** node graph (service map), flame graph (profiles), funnel, geomap.
- **Field config,** shared by every visualization:
  - unit (duration, bytes, percent, count, and currency from `Uom`), decimals, min and max, and no-value text
  - thresholds, absolute or percent, with colours
  - value mappings by value, range, regex or null
  - display name
  - data links: templated URLs to Moqui screens, such as the order find screen filtered by the clicked status

  Overrides apply these by field name, regex or type. Colours come from the theme's `--q-*` variables, as `get#ChartConfig` does today.
- **Suggestions.** The visualization picker ranks types by the frame's columns, after OSD Explore's rules and Grafana's suggestions:
  - one number gives a stat
  - time plus a number gives a time series
  - a category plus a number gives a bar chart
- **Widgets.** `m-report-panel` generalizes `m-chart` and `get#ChartConfig`: a frame and a field config go in, and a Chart.js config comes out. Its TUI twin draws stat tiles, tables, and sparkline and braille time series, and reuses the `timeline` and `tree` components.

### Dashboards

- **Model:** a map of panels, plus a separate layout tree (Grafana's v2 shape). The same panels can then be laid out again for the screen, a PDF or the TUI.
  - Layouts: a 24-column grid, an auto grid, collapsible rows, and tabs.
  - Repeating a panel or row for each value of a variable comes later.
  - A panel is either stored by value, or a reference to a library panel.
- **Variables:**
  - Kinds: query (terms over a dataset field), custom list, constant, interval, text box, ad hoc filters, and group-by.
  - They chain in order, and can take several values or "All".
  - They sync to the URL (`var-status=OrderApproved`) along with the time range, so a link reproduces the view.
- **Time:**
  - A picker for relative and absolute ranges, with recent ranges.
  - Refresh intervals.
  - Per panel: a time override, a shift, and a comparison.
  - A cursor shared across charts.
- **Annotations** come from the annotations index, and from annotation queries over any dataset that map fields to time and text. They show as markers and bands on time series. Business dashboards use them for price changes, promotions and holidays.
- **Library panels** are the one widget registry: a saved panel used on dashboards and on app landing pages.
  - Each one has a "used in" list.
  - An edit reaches every user of the panel; unlinking makes a copy.
- **Versions:** every save makes one, with a note, a diff and restore.
- **Organizing:** folders, tags and stars, and permission by user group.
- **UI** (after Grafana):
  - **Header:** folder and title, star, variables, time picker, refresh, share, edit.
  - **Panel menu:** view, edit, share, explore, inspect (data as formatted or raw CSV, requests and stats, JSON), duplicate, new alert rule, remove.
  - **Edit mode:** an options pane for the selected item, a search over all options, and save with a note.
  - **Keys:** `t-` zoom out, `t←`/`t→` shift the range, `e` edit, `v` view, `i` inspect, `d k` kiosk. The TUI uses the same keys.
  - **Kiosk mode and playlists** for wall displays, such as a NOC or a warehouse shipping board.

### Explore

- **Layout:** Discover's, on any dataset:
  - dataset and time pickers
  - a query bar (the Lucene subset in [`parse.rs`](../crates/moqui-core/src/search/parse.rs)) with history, and filter pills
  - a field sidebar: fields by kind, top values from a Terms aggregation, and "visualize" for a field
  - a count-over-time histogram
  - a document table: expand to fields and JSON, choose columns, and filter for or out from a cell
- **Context:** the lines around a log line, through cursor paging.
- **Split view:** two panes side by side with linked time, to compare two ranges, versions or instances (requirement 4).
- **Correlations** between signals: from a span to its logs, from a log to its trace, from an error to sample traces, and from a field value to a Moqui record screen.
- **Saving:** save a search, or "add to dashboard" as a panel.
- **Queryless browsing** of spans by service and operation, after Grafana's Drilldown apps: small RED charts, and clicking a label breaks the view down by it.
- **Monitor's screens:** Logs and Metrics are Explore presets; Traces keeps its waterfall.
- **Two stages:**
  - **M2, the search core,** for the Logs screen and the Visit request lists, because Log Viewer and the hit screens go in M2. It has the dataset and time pickers, the query bar, pills, the document table with expand, paging and sort order, context, and export.
  - **M3, the rest:** the field sidebar, the histogram, split view, correlations, saved searches, and DataDocument datasets.

### Alerts

The rule model follows Grafana's.
- **Rule:**
  - A report query, the same as a panel's. "New alert rule" in the panel menu fills it in.
  - A reduce to one value per series.
  - A condition: above, below, within or outside a range. An optional recovery threshold stops flapping.
  - Each series or row is its own alert instance.
- **Evaluation:**
  - A ServiceJob runs the rules each minute, grouped by interval.
  - Normal goes to Pending, then to Firing after the pending period. Firing goes to Recovering, then to Normal after keep-firing-for.
  - No Data and Error are states of their own and never count as healthy (requirement 2).
- **Routing:**
  - Labels from the rule and the series route an alert to contact points: NotificationMessage topics (the header notifications), email through `EmailTemplate`, and a webhook.
  - Notifications are grouped by labels, with group-wait and repeat intervals.
  - Silences (one-off, by labels) and mute timings (recurring, such as weekends) hold notifications back.
- **History:** state changes are written to the annotations index, so they show on dashboards. Each rule links back to its panel.
- **Business rules** use the same model, for example daily sales below a floor or overdue receivables above a ceiling.

### Scheduled reports and sharing

- **Reports** extend `ScreenScheduled`, or use a sibling entity if its fields do not fit. `ScreenScheduled` already gives a cron schedule, email to a user or group through `EmailTemplate`, saving to a location, and skipping when empty. A report adds:
  - one or more dashboards, each with its own time range and variable values
  - PDF, portrait or landscape, in the dashboard's grid or one panel per row, with an optional appendix of table data
  - CSV for each table panel, and xlsx
  - a PNG of the dashboard inline in the email body, with a link to the live view
  - preview, send now, and pause
- **Rendering** uses the existing Chromium engine.
  - The print document is self-contained: the server computes the frames, Chart.js and the data are inlined, and animation is off.
  - The engine waits for a "rendered" flag before `Page.printToPDF` or `Page.captureScreenshot`.
  - No request leaves the page, as today.
- **Sharing:** a link with the time range made absolute, and an embed of one panel or dashboard in another screen. Public and anonymous links are deferred, because ERP data on a self-hosted server should not get a bearer URL by default.

### Definitions

- **One saved-definition entity** holds dashboards, library panels, datasets, saved searches, alert rules and report schedules.
  - Fields: type, id, title, JSON body, owner, user group, the component that shipped it, and a read-only flag.
  - Version rows keep every save.
  - A reference table records what each definition uses (named references, as in OSD's saved objects). It feeds the "used in" lists and export.
- **Shipped definitions:** components ship one JSON file per definition in a definitions directory. They load at start as read-only, and "save as copy" makes an editable one.
- **Export and import** move a definition together with what it references, remapping ids.
- The entity package and the directory name are settled in M3.

### Security and cost

- **Every query runs as the viewer,** through the dataset's permission and row filters.
  - A saved panel never widens access.
  - A report runs as its owner, and the owner's access is checked at each run.
- **Limits:** row, bucket and time limits on every query. Exports are audited.
- **Assist** reads frames through the same checks, and never gets more than the viewer could see.

## Implementation plan

Light plan. Each milestone ends with tests and an overhead measurement against the [Profiling](Profiling.md) speed tests (artifact spans sit on the hottest path).

**M1: foundation and traces**
- Check the crates with `cargo tree`, add the License-Review row, and add the `telemetry` feature.
- Add the `moqui-core::telemetry` module and the telemetry config in `MoquiDefaultConf.xml`.
- Set the resource: service name, instance id, version, and a git revision emitted by `build.rs`.
- Add the request, artifact, SQL and job spans, `traceparent` propagation (HTTP in, `CallJob`, outbound, the qvt page), sampling (every request root kept), and the local exporter to `otel_spans`.
- Add the Monitor app root, the Traces and TraceDetail screens with `m-trace-waterfall`, and their TUI twins.
- Tests:
  - span tree shape for a screen, service and entity call
  - propagation across `CallJob`
  - Postgres-backend index and search (like `crates/moqui-host/tests/search_logs_postgres.rs`)
  - a failed export does not fail the request

**M2: metrics, logs and panels; retire hits and log shipping**
- Add the SDK instruments (delta temporality), metric documents and rollups, and the retention job. Measure the minute-document volume.
- SearchFacade:
  - `Avg`, `Percentiles` and the bucket merge
  - fixed and auto intervals, extended bounds, partial buckets
  - Missing and Other buckets
  - ValueCount, Stats, Median, TopHits
  - Histogram, Range and Filters
  - cursor paging
- Frames, report queries with tabifying, the core transformations, and datasets for the telemetry indexes.
- `m-report-panel` with time series, stat and table (with column sort and CSV and xlsx export), and their TUI twins.
- The slow-frame rule and counter, moved from `hits.rs`. Honour `artifact-stats` with a deprecation warning, and `track-artifact-hit`.
- Move `LineSubscriber` export to the OTel logger.
- Add the dashboard, Services and Artifacts dashboards. The dashboards are JSON files in the tools component, rendered read-only until M3 brings the saved-definition entity and the editor.
- Add Explore's search core, with the Logs and Metrics screens on it.
- Move every reader in the [Retired and replaced](#retired-and-replaced) tables. Then remove ArtifactHit, ArtifactHitBin, the System Artifact Summary, Artifact Bins and Log Viewer screens, and the hit and log-ship code.
- Tests:
  - metric documents per interval and rollup totals
  - percentiles and each new aggregation on both backends
  - tabifying nested aggregations, and the transformations
  - log trace ids and retention
  - each Log Viewer function in the Logs screen, on both backends: query string with time range, paging and both orders, the exception dialog, and the user link
  - Visit detail and Visit list from request spans, including a visit whose traces were not sampled
  - Artifacts dashboard totals and slow counts that match a known run of screens and services

**M3: the Dashboards app**
- Saved definitions with versions and references; loading definitions from component files; export and import.
- Dashboards:
  - layouts
  - variables with URL sync
  - the time picker and per-panel time
  - library panels
  - the editor with option search
  - the inspector
  - kiosk mode
- The annotations index, the REST endpoint for deploy markers, and annotations on time series.
- The rest of Explore: field sidebar, histogram, split view, correlations, saved searches.
- DataDocument datasets with row filters, index-wide datasets with the across-type aggregations, the generic report query service and REST transition, and the service source.
- General document search in Explore. Then remove `System/DataDocument/Search.xml` and point the System dashboard's Data Documents box at Explore.
- The rest of the first visualization set.
- A business dashboard on `mantle_sales` (sales by month, orders by status, top products), shipped from mr-mantle as the check that business reporting works.
- Tests:
  - relative time parsing
  - a dataset the viewer may not read is refused, and row filters apply
  - a shipped definition is read-only, and "save as copy" works
  - export and import keep references
  - a search of the `mantle` index returns Party and Product hits with their titles and `DataDocumentLink` links, and opens a document
  - a `moqui test-client` spec on a dashboard

**M4: errors**
- Fingerprinting, the atomic `otel_errors` upsert, and the Errors screens, linked from spans, logs and the dashboard.
- Test that fingerprints stay stable across message parameters and line shifts, and that counts from two instances add up.

**M5: external sources**
- The OTLP/HTTP receiver (keys, limits, redaction, partial success, cumulative-to-delta), the optional OTLP exporter, and the Sources screen.
- Tests:
  - a `moqui test-client` spec posting protobuf and JSON fixtures
  - a manual run with an OTel Collector or demo app sending to the server
  - export to a collector

**M6: alerts and versions**
- Alert rules from report queries:
  - the evaluation states, including No Data and Error
  - routing to contact points
  - silences and mute timings
  - state history in annotations
  - "New alert rule" from a panel
  - one business rule as an example
- Release records (from the resource version and git revision), written as annotations, with a version-to-version comparison of RED metrics in Explore's split view. This is the first piece of requirements 1 and 4.

**M7: scheduled reports and sharing**
- Dashboard report schedules on `ScreenScheduled`, PDF and PNG through the self-contained print document, and CSV and xlsx per table panel.
- Links with an absolute time, embeds, and playlists.
- Tests:
  - a report renders PDF and PNG with no network request
  - an empty report is skipped
  - the CSV matches the panel's frame

**Later, as the requirements call for them:**
- **Server-JS profiling.** Instrument generated JS at function boundaries and at selected block and loop boundaries. Handwritten JS goes through an AST transform (for example `@babel/parser`, `@babel/traverse` and `@babel/generator` at build time).
  - Record through bounded hooks: call counts, inclusive and self elapsed time, host-call time, and sampled block counts. Keep recursion, exceptions, returns and async semantics intact.
  - Map stable function and location ids to the JS/XML source and the build, and link selected spans and errors to the active trace. Never capture arguments or locals.
  - Gate detailed instrumentation by script or request, and by a time and overhead budget. These timings are wall time, not CPU samples.
  - Native `pprof` (Rust on Linux) cannot resolve interpreted JS frames. True JS CPU or line sampling needs a QuickJS-NG engine-hook prototype or a verified runtime profiler.
- **Browser RUM** through the same OTLP receiver, and `rrweb` replay that is opt-in, masked and bounded.
- **Business operations.** Stable operation names, with commit, rollback, retry and pending outcomes taken from the transaction hooks (`on_commit` and `on_rollback` in `transaction/manager.rs`). Use authorized correlation references, never document contents or metric labels.
- **Regressions and incidents.** Persist baselines, regressions, incidents and evidence. Use deterministic comparisons and cause ranking, with sample and data-quality gates and explicit uncertainty. Incident, release and operation screens keep the tenant, build, runtime and time context, with separate diagnostic and recovery permissions and audited access.
- **Recovery.** A durable state machine (policy, decision, execution, verification) with a deployment-controller adapter.
  - Persist each idempotent action before running it, with one active action per scope.
  - Require compatibility checks and approvals, with cooldowns, deadlines and an emergency stop.
  - Verify against business health.
  - Advisory by default. Never reverse committed business transactions automatically.
- **Partitions and gRPC.** Day partitions on the Postgres backend, and the gRPC receiver.
- **Dashboards, later:**
  - repeating rows and panels
  - node graph, flame graph, funnel and geomap
  - grouping log lines into patterns
  - an agent-traces view over `gen_ai.*`
  - Git sync of definitions
  - public links, if they are wanted

Completion of the later stages is gated on: correlated failures, tenant and privacy isolation, tolerance of a telemetry outage, measured overhead, regression detection, and safe, retried and failed recovery tests. Other signal types and specialized infrastructure wait until they are needed. No extra server is required by default, and an OpenTelemetry Collector stays optional.


## Implementation verification log

Implementation and deterministic acceptance checks ran Oct 26–27, 2026. The table below records tested functionality for M1–M7, rather than declaring every milestone gate complete. Performance measurements cover the committed first-pass and final completion-wave snapshots; isolated historical milestone baselines were not captured. This distinction limits the original per-milestone acceptance claim, as detailed under remaining work.

Work was delegated with one owner per shared lane, at most three subagents plus the primary:

| Owner | Delivered scope |
| --- | --- |
| Primary | Manifests, configuration, registration/security seeds, print rendering, shared integration, acceptance, benchmarks and upgrade notes |
| Telemetry | Context/propagation, instrumentation, local/OTLP exporters and receiver, rollups/retention, error normalization, shutdown and legacy persistence retirement |
| Query/report | Search backends, authorized datasets/queries, frames/transforms, saved definitions/references, alerts and business/document acceptance |
| Web/UI, then TUI | Monitor/Dashboards screens, browser widgets, shipped dashboards, reader migration, client specs, native components and screen traversal |

Replacement readers passed PostgreSQL and live OpenSearch acceptance before legacy reader/persistence removal. Browser and TUI consumers share the authorized reporting engine. Every new qvt view has native traversal coverage; specialized waterfalls, live logs, panels and dashboards use compiler-free registered components. Completion-wave native acceptance below also verifies editing, query/filter controls, inspector steps, playlists and bucket interactions.

### Functional acceptance

| Milestone | Acceptance evidence |
| --- | --- |
| M1 | SDK/context tests cover worker-queue and job propagation, buffered/streaming HTTP context, zero child sampling with every request root retained, error/slow promotion, descendant suppression, provider lifetime, queue saturation and exporter failure isolation. HTTP acceptance verifies real request trees and ingest/auth boundaries. Browser/TUI trace waterfalls and job links pass. |
| M2 | PostgreSQL and live OpenSearch reader parity, Visit counts batched once per page, normalized logs/boot logging and retention, known-workload minute metrics, 1,005-point retry-safe hour/day rollups, histogram compatibility, all planned aggregation fixtures and 10,025 equal-timestamp cursor traversal pass. Legacy readers/persistence were then retired. |
| M3 | Dataset/group/row restrictions run before mixed-type totals, paging, document retrieval, embeds and exports on both stores. Shipped read-only definitions, copy/version/import/reference/cycle handling, transformations, completeness notices and the Mantle sales dashboard pass. Browser editors, Explore, inspector and saved definitions pass; all new screens traverse in TUI. |
| M4 | Stable normalized fingerprints survive parameter/line changes; concurrent atomic occurrence updates pass on PostgreSQL and live OpenSearch. Errors navigate to authorized logs and traces; dashboard error panels use groups. |
| M5 | Strict key/revocation/owner checks, limits, compression, redaction, partial success and cumulative reset handling pass. A real OpenTelemetry Collector sends JSON and raw protobuf for all three signals; outbound traces/logs/metrics reach a live Collector. Source/key and loss-status screens have native traversal coverage; HTTP tests exercise their underlying key and exporter behavior. |
| M6 | Controlled-clock Pending/Firing/Recovering/Normal/No Data/Error, hysteresis, grouping/repeat/silence/mute behavior, restart persistence and four concurrent evaluators pass. Incomplete/missing data cannot establish health. Owner authority, notifications, annotations and version comparison paths pass. |
| M7 | Real Chromium PDF and PNG wait for completed charts and reject report errors while blocking network access. CSV/xlsx use the same authorized frames. Empty-report suppression, MIME/CID PNG delivery, owner revocation, concurrent/live/stale/restarted schedule leases and independent destination failures pass. Browser absolute-time links, embeds and playlists pass. |

Final serial acceptance commands (run from the repository root, with local PostgreSQL/shared-memory and Chromium access):

```sh
cargo test -p moqui-core --features telemetry --lib --offline -- --test-threads=4
cargo test -p moqui-tui --offline
cargo test -p moqui-host --lib --offline -- --test-threads=1
cargo test -p moqui-host --test monitor_edge --test report_monitoring --test report_alerts --test report_schedule_lock --test telemetry_rollup --test monitor_readers --test monitor_tui --test search_logs_postgres --test report_documents --test mantle_search --no-fail-fast --offline -- --test-threads=1 --nocapture
cargo test -p moqui-host --test tui_screens --offline gallery_screens -- --exact --nocapture
cargo --offline lint
cargo --offline lint-min
```

- Core final controlled-concurrency run: **2,003 passed, zero failed, two ignored** (67.06 s). The ignored cases are a manual timing benchmark and the live OpenSearch test, which passed separately. Earlier full runs exposed an existing large-Git-object transfer reset (isolated retry **1/1**, 2.04 s) and a paused-schedule regression. The stronger test now commits discovery and pause separately, asserts the locked reread and verifies no lease-clock query occurs. It exposed report definition/owner/pause fields attached to `ScreenScheduledLock`; these were corrected onto `ScreenScheduled`, leaving runtime lease fields on the lock. The corrected regression passes (**1/1**, 0.06 s). Earlier failures are retained in diagnostic logs.
- Host unit suite: **214 passed**, 55.47 s, serial to avoid exhausting macOS shared-memory slots.
- Native TUI: **163 passed**, one ignored documentation example. Monitor traversal covers all 23 new views plus specialized inspector/download/trace paths.
- Targeted real PostgreSQL/HTTP/scheduling/Chromium acceptance: **14 passed**, five external-service tests excluded from this command and run separately below; 117.27 s including compilation.
- After the scheduling schema correction, `cargo test -p moqui-host --test report_schedule_lock --test monitor_edge --test monitor_tui --offline --no-fail-fast -- --test-threads=1 --nocapture` passed **6 tests**, with the two Collector cases already tested separately. This rerun covers real committed schedule state/restart/concurrency/revocation, request/ingest boundaries, isolated PDF/PNG and all new native views.
- Gallery keyboard traversal and regenerated native snapshots passed; the subsequent run **without** `UPDATE_GOLDEN` also passed (7.08 s).
- Full workspace `cargo --offline lint` and minimal-feature `cargo --offline lint-min` passed with warnings denied (13.77 s and 6.17 s respectively, final post-correction reruns).

Server JavaScript specs passed through `target/develop/moqui test framework --spec ScreenHtml,ScreenQvt --db none` (**29/29**, 0.52 s) and `target/develop/moqui test tools --spec DataView` (**1/1**, 12.98 s, isolated fresh database). Browser reruns use `target/develop/moqui test-client`: `webroot --offline --spec WebrootReport` **18/18**, `webroot --offline --spec WebrootAssistCanvas` **16/16**, and `tools --serve --spec Monitoring` **6/6** passed. No Node test runner was used. Assist replay checks also pass without live model calls; recorded model responses were preserved.

Final live-service reruns passed against disposable **OpenSearch 3.0.0** and **OpenTelemetry Collector 0.130.0**, using the already-built final test executables serially:

- OpenSearch replacement readers **1/1** (13.38 s), retry-safe rollups/retention **1/1** (13.54 s), mixed-document permissions/totals/retrieval **1/1** (12.22 s).
- Live OpenSearch aggregation/compatible histogram/cursor/atomic-update/schema-preservation fixtures **1/1** (2.64 s), including 10,025 equal-timestamp records and concurrent increments.
- Collector inbound JSON and raw protobuf for traces/logs/metrics plus outbound all-signal transport **2/2** (1.94 s).

Reproduce with disposable services on the stated ports; these tests use isolated prefixed indices and scratch databases:

```sh
MONITOR_READER_OPENSEARCH_URL=http://127.0.0.1:19200 cargo test -p moqui-host --test monitor_readers --test telemetry_rollup --test report_documents --offline -- --ignored --test-threads=1 --nocapture
MONITOR_OPENSEARCH_URL=http://127.0.0.1:19200 cargo test -p moqui-core --features telemetry --lib --offline search::opensearch::tests::monitoring_aggregations_and_cursor_on_live_opensearch -- --ignored --exact --nocapture
MONITOR_COLLECTOR_URL=http://127.0.0.1:14318 cargo test -p moqui-host --test monitor_edge --offline -- --ignored --test-threads=1 --nocapture
```

The Collector-forwarding test also creates/removes its own local container (`--pull=never`); have the pinned Collector image available. Dedicated acceptance containers were stopped after the checks; no production service or user data was used.

Evidence logs live under `target/monitoring/` (local build artifacts, not committed): `core-final.log`, `core-git-retry.log`, `core-limited-final.log`, `core-limited-initial.log`, `core-paused-final.log`, `schedule-schema-final.log`, `server-screen-js-final.log`, `server-tools-js-final.log`, `final-acceptance.log`, `tui-tests-final.log`, `host-lib-final.log`, `gallery-final.log`, `report-browser-final.log`, `assist-canvas-browser-final.log`, `tools-browser-final.log`, `report-opensearch-acceptance.log`, `report-documents.log`, and `collector-tests.log`, and `final-live-acceptance.log`. Counts, measurements and reproduction commands are retained here so they survive build-artifact cleanup.

### Performance measurements

Results: `target/monitoring/benchmark-summary.json`, sixteen per-run logs, and `benchmark-runs.json` (all exit 0). Compiler: rustc 1.99.1 (58cf8f9c3, 2026-10-13), optimized incremental `develop` profile.

**The M1–M7 labels below are seven paired repeats of the committed first-pass implementation, before the completion wave; they are not isolated staged baselines or incremental milestone costs.** Runs were serialized, alternating off/on order. Telemetry-on uses full sampling (1.0); its overhead remains substantial for small operations.

The service fixture uses embedded PostgreSQL over a Unix socket, an entered context with authorization disabled, one ordinary plus one reserve connection, warmup 32, and 1,000 calls × 5 batches. The table selects the “one transaction” rows; values are median microseconds per call. “DB read” is the complete uncached entity PK lookup, including its instrumentation, with exactly one SQL statement per call. Echo executes no SQL.

| Repeat | Echo off → on, µs | DB read off → on, µs | Rust live growth off → on, MiB | Peak RSS off → on, MiB | On exported / dropped / failed |
|---|---:|---:|---:|---:|---:|
| M1 | 8.97 → 19.35 | 18.35 → 38.61 | 1.036 → 4.697 | 48.75 → 52.77 | 110,919 / 0 / 0 |
| M2 | 8.85 → 19.35 | 18.31 → 38.61 | 1.036 → 4.697 | 49.62 → 52.50 | 110,919 / 0 / 0 |
| M3 | 8.94 → 19.20 | 18.24 → 39.06 | 1.036 → 4.697 | 49.62 → 52.80 | 110,919 / 0 / 0 |
| M4 | 8.86 → 19.80 | 18.44 → 39.32 | 1.036 → 4.697 | 48.59 → 52.52 | 110,919 / 0 / 0 |
| M5 | 8.94 → 19.54 | 18.26 → 38.38 | 1.036 → 4.697 | 48.61 → 52.58 | 110,919 / 0 / 0 |
| M6 | 9.04 → 18.61 | 18.32 → 38.68 | 1.036 → 4.697 | 49.73 → 52.73 | 110,919 / 0 / 0 |
| M7 | 8.90 → 20.37 | 18.31 → 38.49 | 1.036 → 4.697 | 49.39 → 53.16 | 110,919 / 0 / 0 |

Echo is 2.06–2.29× the telemetry-off latency; uncached DB reads are 2.10–2.14×. Echo allocations rise from 94.0 to 127.2–127.3 per call; DB-read allocations rise from 38.0 to 56.1. Cached entity reads perform 19 allocations and 1,236 allocated bytes per call in both modes. These results are measurements, not a claim that full-sampling overhead is negligible.

“Rust live growth” is the process-wide net Rust allocation change after telemetry flush, using sharded accounting so exporter-thread frees are included; it includes retained caches and instrumentation state. It is not per-call leakage or a steady-state heap bound. Per-row `kept` is a separate producer-thread measurement and should not be interpreted as retained process memory when another thread frees queued objects. Peak RSS is `/usr/bin/time -l` process RSS, including startup, allocator and non-Rust memory; it is not the same quantity as live Rust bytes.

Each enabled service repeat queued and exported 110,919 records: 110,907 spans, 9 metric documents, and 3 logs. The recording sink serialized approximately 132.88 MB (126.72 MiB) of normalized JSON per run, of which about 132.87 MB were spans. Every repeat reported zero dropped and failed records. Telemetry-off exported no records. Metric-document counts are this fixture’s active series and explicit collection/flush, not a production cardinality estimate.

Representative JavaScript/service and screen costs (three batches, warmup 32; 500 service calls or 100 screen renders per batch):

| Operation | Off, µs/call | On, µs/call | Ratio |
|---|---:|---:|---:|
| noop#Inline (empty actions) | 3.61 | 10.46 | 2.90× |
| echo#Script (the same, a .js file) | 8.35 | 17.15 | 2.05× |
| call#Nested (service-call to set#One) | 11.94 | 27.92 | 2.34× |
| Plain, html (one label) | 18.17 | 40.07 | 2.21× |
| Single.qvt (form-single, 11 fields) | 451.05 | 492.15 | 1.09× |
| List, html, 100 rows | 6136.16 | 6150.81 | 1.0024× |
| List.qvt, 100 rows | 9688.96 | 9731.34 | 1.0044× |

The JS/screen run queued/exported 23,090 records with zero drops/failures: 23,057 spans, 30 metrics and 3 logs, approximately 27.80 MB serialized JSON. Rust live growth was 2.875 → 6.540 MiB; peak RSS was 55.19 → 60.50 MiB. Remote exporter counters were zero in all benchmark runs.

The initial full-sampling run exposed 19,363 dropped records, caller-thread document normalization and repeated scans over all trace decisions. The final implementation queues owned SDK records, normalizes on the bounded worker, amortizes decision expiry scans, and avoids repeated attribute/instrument allocations. The initial echo allocation count was 247.5/call; final is about 127.2/call. All seven final service pairs export every queued record. A diagnostic single-global-atomic memory counter caused contention; final measurements use cache-line-separated per-thread shards. Neither the discarded counter probe nor profiler-perturbed overload runs are used as acceptance timing figures.

Reproduce with the same compiler, preserved caches and no other builds, tests or containers running. Build once, then use the test executable paths printed by Cargo (`HASH` below is the printed hash):

```sh
cargo test -p moqui-host --profile develop --test service_speed --test js_speed --no-run --offline
# Run each mode serially; repeat seven pairs, alternating which mode goes first.
telemetry_enabled=false MONITOR_SPEED_EXPORT=true SERVICE_SPEED_CALLS=1000 SERVICE_SPEED_BATCHES=5 /usr/bin/time -l target/develop/deps/service_speed-HASH --ignored --nocapture --test-threads=1
telemetry_enabled=true telemetry_sample_ratio=1.0 MONITOR_SPEED_EXPORT=true SERVICE_SPEED_CALLS=1000 SERVICE_SPEED_BATCHES=5 /usr/bin/time -l target/develop/deps/service_speed-HASH --ignored --nocapture --test-threads=1
# Repeat this JS/screen command with telemetry_enabled=false, then true:
telemetry_enabled=false MONITOR_SPEED_EXPORT=true JS_SPEED_CALLS=1000 JS_SPEED_BATCHES=3 JS_SPEED_ONLY=service,screen /usr/bin/time -l target/develop/deps/js_speed-HASH --ignored --nocapture --test-threads=1
```

### Gap completion wave after the committed baseline

The Oct 27 completion wave starts from commit `d6dc52b2` (implementation commit `735b3860`). Browser, native and query/backend work have separate subagents; the primary owns telemetry, shared contracts/configuration, scheduling integration, performance measurements and this acceptance log. Later features remain deferred.

| Gap | Current result / evidence |
| --- | --- |
| Native controls and authoring | Implemented; **176 native tests pass**, including query/filter editing, nested layouts, table sorting/links, authorized step snapshots, structured authoring/JSON preservation and playlist rotation/pause. Focused native Clippy passes. **Real HostLink acceptance passes** (9.43 s): 23 screens, all seven component types and actual snapshot/editor/playlist interactions. Five additional canonical Missing/Other/sorted-filter control tests pass; native Clippy passes. Split-series sparklines preserve numeric/hidden groups, missing time coordinates and original row indices, with separately labelled duplicate coordinates. Logs: `gap-tui-metadata-final.log`, `native-host-gap-final.log`, `native-bucket-gap-tests.log`, `native-gap-clippy.log`. |
| Exponential histograms | **46 telemetry tests pass**, including exact large counts and cumulative integer sums above 2⁵³, bounded scale downconversion, signed buckets, zero-threshold reconciliation, retry-safe hour/day populations and cumulative delta/reset/replay handling, zero-duration unknown-start baselines and same-start decreases across hour boundaries. Final PostgreSQL/OpenSearch hour/day, retention and retry acceptance passes **2/2** (16.45 s), including exponential populations and 1,005-observation source paging. Logs: `gap-priority-tests-final.log`, `gap-refresh-readers-final.log`. |
| Streaming request duration | **Passed real middleware acceptance** (0.08 s): root absent at headers, full delayed-body duration, exact bytes, error/cancellation status and exactly one retained root per request with zero child sampling. Log: `http-stream-gap-tests.log`. Transfer-worker context propagation also passes (0.02 s), including Some/None restoration and no guard across async suspension. Final full host suite **217/217 passes** (56.03 s); log `gap-host-completion-final.log`. Final streaming regression passes separately (0.13 s), `gap-http-priority-final.log`. |
| Browser authoring/filter clicks and OTLP specs | **Closed: 28 component specs and eight served workflows pass** (1.44 s / 23.14 s). Real multi-query/join authoring saves/renders, document and canonical Missing/Other/literal-label chart clicks create typed pills, filtered CSV matches, and two-dashboard report composition saves/previews. Source keys ingest JSON/raw protobuf for all three signals. Split-series plots align time/gaps and source rows; generated boolean bucket flags do not become series, while explicit user roles are preserved. Stat/gauge/barGauge cards use the same resolved integer metrics and split groups, preserving null/zero values and bounded per-series display. Logs: `report-gap-browser-final3.log`, `tools-gap-browser-final5.log`. |
| Exact cross-type aggregation | **Passed PostgreSQL and live OpenSearch** mixed-document acceptance (2.14 s / 14.14 s), covering global Avg/Stats/Median/Percentiles/Cardinality/TopHits/histogram populations, exact Other requery and collision-safe Missing/Other bucket predicates, with type/group/row policy applied before totals. Logs: `backend-gap-pg.log`, `backend-gap-os.log`. |
| Nested partial populations / filtered exports | **Closed: real PostgreSQL acceptance passes**, 3.23 s. Partial observations outside a selected parent no longer mark it incomplete; selected partial observations produce exact counts. Saved/runtime/dataset-scoped/disabled/negated/identity-widening pills match render/preview CSV under row policy, and sorted/filter/renamed bucket provenance replays the same authorized rows. Log: `gap-report-filter-final.log`. |
| Alert transport | **Closed: real SMTP/webhook acceptance 2/2 passes**, 4.43 s. Authenticated SMTP has a 451 failure followed by MIME delivery; real HTTP covers 503/success and independent contacts. Missing credentials, per-delivery URL/header rotation, persisted retry/repeat state across restart and revoked-owner blocking pass. Fixed email success-ledger handling: a saved email row without a delivery message ID remains retryable. Logs: `alert-wire-gap-tests.log`, `alert-m6-gap-final.log`. |
| Semantic row-policy acceptance | **Closed: configured pgvector positive-ranking test passes**, 2.20 s. The unfiltered nearest document is denied; authorized semantic ranking returns only the permitted population. Uses isolated PG17/pgvector schema, cleaned after the test. Log: `gap-vector-final.log`. |
| Request-root retention under overload | The real full-sampling PostgreSQL burst exposed **321/416** roots retained with 3,646 observable drops. Bounded flush enqueue/ack now shares a five-second deadline. **46 telemetry tests pass**, including hard record/byte bounds, external records excluded from the reserve, order-preserving mixed batches and mandatory-root protection during oversized tail promotion. **Final load rerun passes: 416/416 roots in every enabled configuration**, separately from observable child/metric drops. The performance fixture asserts this rather than inferring root retention from total exports. Diagnostic: `pre-priority-postgres-full.log`. |
| OpenSearch telemetry export throughput | Real-store probe exposed ~1 s refresh waits for each stable-ID bulk (3.89 s drain for 1,248 records). Added explicit telemetry-only deferred-refresh writes; ordinary SearchFacade writes/rollups retain their original visibility contract. **73 search tests pass**, including both live OpenSearch fixtures; replacement readers, both-store rollups and all six edge/Collector/print tests pass after this change. Final 1,248-record probe drain is 15.06 ms. Logs: `gap-refresh-core-final.log`, `gap-refresh-edge-final.log`, `gap-refresh-readers-final.log`. |
| Real-store/transport overhead | **Closed: all 19 frozen-source optimized runs pass**, including both stores, authenticated one/four-worker HTTP, disabled/roots/full sampling, local persistence with/without OTLP, all three transport signals and current Profiling off/on pairs. Full results and the measured PostgreSQL full-sampling capacity limit follow. |

Final completion-wave Rust validation: core **2,021 passed, zero failed, three ignored** (52.26 s), host **217 passed** (56.03 s), native **176 passed, one ignored documentation example**. Full workspace `cargo --offline lint` and `cargo --offline lint-min` pass (14.37 s / 6.22 s). The three ignored core cases are a manual benchmark and the two opt-in live OpenSearch regressions; both live cases pass within the 73-test search rerun. The core catalog test requires the declared optional `mr-l10n` component: acceptance used upstream commit `a546b63` from an ignored scratch checkout through a temporary component symlink, removed after testing; no addon was permanently installed. Logs: `gap-core-priority-final.log`, `gap-host-completion-final.log`, `gap-tui-metadata-final.log`, `gap-priority-lint-final.log`, `gap-priority-lint-min-final.log`.

Final all-inclusive `monitor_edge` acceptance passes **6/6** (13.74 s, 46.61 s including compilation), including Collector forwarding and outbound transport for all three signals, request/ingest boundaries, network-isolated Chromium PDF/PNG and scheduled export/owner checks. The subsequent **73/73 search tests** include both live OpenSearch fixtures (5.47 s), covering aggregation/cursor/atomic-update/schema preservation with 10,025 equal-timestamp rows and deferred-refresh stable-ID replacement/visibility. Replacement readers plus both-backend rollups also pass **5/5** (8.01 s + 16.45 s). Logs: `gap-refresh-core-final.log`, `gap-refresh-edge-final.log`, `gap-refresh-readers-final.log`. Server JS passes **29/29** screen specs (0.50 s) and **1/1** DataView spec (11.31 s) through `moqui test`; logs `gap-server-js-final.log`, `gap-tools-js-final.log`.

The first serial rerun exposed test-environment issues, retained in diagnostic logs: exhausted macOS shared-memory slots from dead PostgreSQL processes, a missing optional translation fixture, a TCP delivery timing assertion, and overlapping phrase-search fixtures. Only verified dead, unattached PostgreSQL shared-memory headers owned by the current user were removed; no active database or data files were changed. The TCP probe now allows a bounded delivery window. An overbroad `--ignored` invocation also selected the vector case without its required environment; the explicitly configured positive pgvector rerun passed (2.20 s, `gap-vector-final.log`). No failing case is silently counted as acceptance.

### Final completion-wave performance (Oct 27)

All **19 optimized runs pass**: two setup probes, twelve HTTP/store configurations, one nine-case transport run and four Profiling off/on runs. Source was frozen; builds, other tests and subagent runtimes were stopped. Compiler: rustc 1.99.1 (58cf8f9c3); profile: `develop`, optimized/incremental, caches preserved. Host: Apple M1 Pro, eight logical CPUs, 32 GiB RAM, macOS 27.0.1. The earlier repeat is diagnostic corroboration (`completion-repeat1-results.json`), not a historical milestone baseline. Final raw data: `target/monitoring/completion-benchmark-results.json` and `completion-*.log`.

The HTTP fixture signs in as the seeded administrator with authorization enabled. A saved index-wide dataset spans two DataDocument types and applies a server-owned `scope=ALLOWED` row policy: **32 of 128 rows** are visible, with 96 denied rows excluded before totals. Each configuration warms eight requests, then runs **200 sequential requests followed by 200 requests with four workers**, without an intermediate drain, plus a separate 16-request allocation pass. Every request uses a fresh loopback TCP connection and consumes the authorized dashboard HTML. This measures server/render/query cost; it does not measure browser chart painting. The demo screen quota is raised to 100,000 only in the disposable fixture database; authorization and row policy are never disabled during HTTP timing.

PG is embedded PostgreSQL over its normal Unix socket. OS is disposable OpenSearch 3.0.0 (512 MiB JVM heap). Both local OpenSearch and Collector services remain running consistently for paired runs and receive only fixture traffic; the unused vector container is stopped. `tee` adds the production outbound OTLP path to Collector 0.130.0 with the committed `fixtures/monitoring/collector-performance.yaml` nop pipelines. Fresh prefixed OS indices are removed after each final run. Measurements exclude startup latency; peak RSS includes startup. C allocations, database/JVM/Collector memory and storage amplification are not captured by Rust allocation counts.

Latency below is **p50 / p95 / p99 in ms** (200 observations per worker scenario). The four-worker phase includes backlog left by the sequential phase. These short-run quantiles do not establish a production tail-latency guarantee.

| Store/output | Mode | One worker | Four workers |
| --- | --- | ---: | ---: |
| PG/local | off | 3.196 / 3.836 / 15.679 | 4.807 / 7.260 / 12.947 |
| PG/local | roots | 3.701 / 4.051 / 12.478 | 5.616 / 6.712 / 12.901 |
| PG/local | full | 3.860 / 4.534 / 5.992 | 5.597 / 7.058 / 17.984 |
| PG/tee | full | 3.895 / 4.670 / 6.140 | 5.437 / 6.705 / 17.246 |
| PG/tee | roots | 3.657 / 4.069 / 15.728 | 5.824 / 12.001 / 15.580 |
| PG/tee | off | 3.140 / 3.557 / 12.473 | 4.774 / 5.635 / 11.835 |
| OS/local | off | 5.475 / 6.788 / 10.436 | 7.396 / 8.770 / 15.734 |
| OS/local | roots | 6.787 / 7.696 / 17.583 | 8.873 / 11.252 / 15.061 |
| OS/local | full | 7.135 / 10.004 / 19.544 | 9.828 / 15.823 / 19.652 |
| OS/tee | full | 6.907 / 10.448 / 12.759 | 9.161 / 14.020 / 16.191 |
| OS/tee | roots | 6.557 / 7.353 / 15.976 | 8.810 / 9.981 / 17.143 |
| OS/tee | off | 7.085 / 8.909 / 21.478 | 7.350 / 12.920 / 18.439 |

Process-wide allocations below come from the separate pass, including exporter-thread allocations/frees; timed passes leave those allocation totals disabled. Retained growth is net live Rust bytes after the timed drain, including caches and tail decisions, not a per-request leak or steady-state bound. Acknowledged span/metric counts include the 400 timed and 16 allocation requests; deterministic metric IDs may be overwritten, so these are write records, not unique physical rows.

| Case | Allocations/request | KiB/request | Retained Δ MiB | Peak RSS MiB | Spans / metrics acked | Core drops |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| PG/local/off | 21390 | 1803.6 | 0.486 | 95.73 | 0 / 0 | 0 |
| PG/local/roots | 28256 | 2491.7 | 2.072 | 99.44 | 416 / 38 | 0 |
| PG/local/full | 36177 | 3599.5 | 14.954 | 131.30 | 10,824 / 38 | 4,984 |
| PG/tee/full | 53472 | 6464.9 | 8.349 | 131.83 | 10,580 / 16 | 5,250 |
| PG/tee/roots | 30044 | 2782.2 | 2.326 | 101.95 | 416 / 38 | 0 |
| PG/tee/off | 21390 | 1803.6 | 0.482 | 95.16 | 0 / 0 | 0 |
| OS/local/off | 21858 | 1943.4 | 0.337 | 96.23 | 0 / 0 | 0 |
| OS/local/roots | 27942 | 2630.5 | 1.592 | 100.56 | 416 / 38 | 0 |
| OS/local/full | 35490 | 3769.3 | 1.812 | 111.16 | 15,808 / 38 | 0 |
| OS/tee/full | 53114 | 6706.1 | 1.707 | 123.95 | 15,808 / 43 | 0 |
| OS/tee/roots | 29713 | 2921.3 | 1.587 | 101.48 | 416 / 38 | 0 |
| OS/tee/off | 21858 | 1943.4 | 0.377 | 96.52 | 0 / 0 | 0 |

**Every enabled configuration preserves all 416 HTTP roots**; disabled configurations export none. All local export-failure and remote drop/failure counters are zero. Tee remote acknowledgements match local exported counts. In roots mode, queued child records intentionally omitted by sampling are not loss; `queued` therefore exceeds `exported` without incrementing drops.

**Capacity limit:** full sampling overloads this PostgreSQL persistence fixture: 4,984 records drop locally and 5,250 with the tee, while all request roots survive. These are excess child/ordinary telemetry records; the tee case also loses one metric collection under pressure. Collection/minute boundaries account for some metric write-count variation between cases. The lower full-sampling request median does not mean that recording every child is cheaper: bounded admission is discarding work. Root reservation fixes the prior 321/416-root failure without expanding the hard queue bounds. Roots mode has zero drops on both stores; full sampling has zero drops on OpenSearch in this run. This is measured saturation, not a lossless-throughput claim. Configure child sampling and capacity for the installation.

| Enabled case | Timed drain ms | Local write-batch p50 / p95 ms |
| --- | ---: | ---: |
| PG/local/roots | 3.76 | 0.623 / 1.535 |
| PG/local/full | 2675.05 | 131.684 / 243.095 |
| PG/tee/full | 2957.96 | 135.264 / 249.618 |
| PG/tee/roots | 6.56 | 0.623 / 1.790 |
| OS/local/roots | 4.57 | 1.904 / 2.895 |
| OS/local/full | 164.37 | 2.980 / 6.954 |
| OS/tee/full | 540.75 | 4.710 / 38.313 |
| OS/tee/roots | 7.68 | 1.815 / 2.906 |

The original stable-ID OpenSearch probe required 3.89 s to drain 1,248 records, with roughly one-second refresh waits. The telemetry-only deferred-refresh probe drains the same population in 15.06 ms. Ordinary write visibility and stable-ID retries pass the live regression; telemetry ACK is explicitly distinct from search refresh.

Outbound transport includes production normalization-to-protobuf and HTTP/Collector acknowledgement; fixture document cloning is outside timing. Each signal/size has one warmup and ten samples. With ten observations, p95/p99 are the observed maximum.

| Signal | Batch | p50 ms | p95/p99 ms |
| --- | ---: | ---: | ---: |
| spans | 1 | 0.602 | 0.680 |
| spans | 64 | 1.492 | 2.070 |
| spans | 512 | 6.897 | 9.911 |
| logs | 1 | 0.479 | 0.547 |
| logs | 64 | 1.153 | 4.203 |
| logs | 512 | 5.855 | 6.348 |
| metrics | 1 | 0.526 | 0.571 |
| metrics | 64 | 1.471 | 2.402 |
| metrics | 512 | 9.200 | 10.610 |

Final Profiling fixture reruns use the recording/normalization sink, keeping their CPU costs separate from the real stores above. Service rows are the one-transaction medians (1,000 calls × five batches, warmup 32); JS/screen rows use three batches of 500 service calls or 100 renders.

| Operation | Off → full, µs/call | Ratio | Allocations off → full |
| --- | ---: | ---: | ---: |
| service echo | 8.93 → 17.40 | 1.95× | 94.0 → 127.2 |
| uncached entity PK read | 17.77 → 38.98 | 2.19× | 38.0 → 56.1 |
| cached entity PK read | 1.65 → 1.72 | 1.04× | 19.0 → 19.0 |
| noop inline JS | 3.61 → 10.28 | 2.85× | 33.0 → 66.4 |
| script service | 8.17 → 16.90 | 2.07× | 88.0 → 121.2 |
| nested service | 11.89 → 27.04 | 2.27× | 97.0 → 166.3 |
| plain HTML label | 17.66 → 38.75 | 2.19× | 122.0 → 216.5 |
| form-single QVT, 11 fields | 436.13 → 471.46 | 1.08× | 1,494.0 → 1,626.5 |
| HTML list, 100 rows | 5,971.67 → 6,054.18 | 1.014× | 28,542.0 → 28,639.4 |
| QVT list, 100 rows | 9,462.78 → 9,498.40 | 1.004× | 25,192.0 → 25,309.8 |

The service pair retains 1.036 → 4.697 MiB net Rust bytes and has 48.77 → 52.58 MiB peak RSS. Its enabled run exports **110,919 records**, zero dropped/failed: 110,907 spans, nine metric records and three logs, 132,881,056 normalized JSON bytes. The JS/screen pair retains 2.875 → 6.540 MiB and has 55.72 → 59.86 MiB peak RSS; enabled exports **23,090 records**, zero dropped/failed: 23,057 spans, 30 metrics and three logs, 27,796,738 normalized bytes. Disabled runs export none; all remote counters are zero for these recording-sink fixtures. Logs are `completion-service_speed-{off,on}.log` and `completion-js_speed-{off,on}.log`. These final pairs do not substitute for missing historical milestone measurements.

Reproduction (substitute executable hashes printed by Cargo; run configurations serially with the same compiler/profile and preserved caches):

```sh
cargo test -p moqui-host --profile develop --test monitor_performance --test service_speed --test js_speed --no-run --offline
# Repeat MODE=off, roots, full; then add MONITOR_PERF_COLLECTOR_URL=http://127.0.0.1:14318.
MONITOR_PERF_MODE=roots MONITOR_PERF_CALLS=200 /usr/bin/time -l target/develop/deps/monitor_performance-HASH authorized_http_with_real_store_and_optional_collector --ignored --nocapture --test-threads=1
# Repeat that matrix with MONITOR_PERF_OPENSEARCH_URL=http://127.0.0.1:19200.
MONITOR_PERF_COLLECTOR_URL=http://127.0.0.1:14318 /usr/bin/time -l target/develop/deps/monitor_performance-HASH collector_transport_batch_latency --ignored --nocapture --test-threads=1
```

Use the committed Collector configuration at `crates/moqui-host/tests/fixtures/monitoring/collector-performance.yaml` mounted read-only in the pinned image, with local port 14318 mapped to 4318. The scratch OpenSearch server disables security and binds only loopback port 19200. Do not target an application database or production cluster.

### Remaining work and validation limits

The identified functional M1–M7 gaps, backend/browser/native acceptance checks and final real-store/transport measurements are complete. The following evidence and capacity limits remain explicit:

- **Historical milestone overhead remains incomplete.** The seven M1–M7-labelled pairs above measured the committed first-pass build, and the completion wave measures the final build. Neither establishes the incremental cost of the original milestones: intermediate baselines were not captured. No historical figures have been inferred or invented, so the original stage-by-stage measurement gate cannot be claimed complete.
- **PostgreSQL full-sampling saturation remains an operational limit.** The measured burst drops 4,984 ordinary records locally or 5,250 with outbound OTLP, while preserving every request root. Roots-only sampling has zero drops on both stores; this OpenSearch full-sampling run also has zero drops. Hard queue bounds and visible losses are intentional. No lossless unlimited-throughput claim or numerical overhead budget is made.
- Measurements cover one macOS host and short warmed synthetic workloads. Authenticated HTTP, an enforced mixed-type row policy, four-worker concurrency and request p95/p99 are measured; the older service/JS fixtures deliberately disable authorization. Production-scale concurrency, long-running retention/rollup load, tenant-specific policy complexity, storage amplification, WAN/TLS Collector transport and multi-host soak remain unmeasured. Rust allocation accounting excludes QuickJS C allocations and database/JVM/Collector memory; peak RSS is reported separately.
- Full child sampling retains substantial cost on tiny operations: the final echo pair is 1.95× and the uncached lookup is 2.19×. Larger screens dilute the fixed cost. Root reservation protects the tested burst, while overload consisting of roots alone or backend failure remains subject to the same bounded storage/queue limits and observable losses.
- Features explicitly listed under **Later** remain deferred, including server-JS block profiling, RUM/replay, operation/incident/recovery state machines, gRPC/day partitions, repeating panels, specialized later visualizations and public links. They remain outside this implementation wave.

Deployment defaults and compatibility:

- Arbitrary attributes/resources remain opaque payloads, with declared typed fields queryable; this preserves bounded mappings. Some runtime process gauges are Linux-specific and unavailable gauges remain missing data.
- New runtime records use `moqui.report` and `moqui.monitor`; component definitions live in `report/` and are read-only until copied.
- Organization restrictions are explicit administrator-authored dataset `rowFilter`/`contextFilters`, added by the server before documents, ranking, totals, aggregations or exports. Automatic translation of existing entity-filter definitions is not assumed. Grant only the intended dataset artifact permissions to business roles.
- Ingestion credentials are source-scoped hashes, separate from ordinary login credentials. Source owners require `MONITOR_INGEST` on every request. Local report file delivery requires `REPORT_SAVE_LOCATION`, granted to ADMIN by default; report recipients do not set execution authority.
- Legacy hit and log persistence has been retired after replacement-reader parity. Historical rows are not migrated or automatically dropped.


### Upgrade notes

- Move any custom component `report-definitions/` directory to `report/`; component loaders now read definitions from `component://NAME/report/`. Shipped definitions remain read-only until copied.
- Back up first and deploy the Rust code together with the framework/tools component changes. The Mantle sales definition/grants are changes in the separate `runtime/component/mr-mantle` repository; the BOSS Insights route is a change in `runtime/component/mr-boss`. Include those component changes when installed.
- Allow the normal entity schema check to create the new `moqui.report`/`moqui.monitor` tables. `ScreenScheduled` gains report definition, execution owner and pause fields; `ScreenScheduledLock` gains the renewable lease expiry/token fields. New installations load the component seed files normally. For an existing installation, review same-ID customizations and load only the new files, using the CLI's `name=value` syntax:

  ```sh
  moqui load location=runtime/base-component/framework/data/TelemetryData.xml
  moqui load location=runtime/base-component/framework/data/ReportSetupData.xml
  moqui load location=runtime/base-component/framework/data/ReportEmailData.xml
  moqui load location=runtime/base-component/tools/data/MonitoringSecurityData.xml
  # Only when mr-mantle is installed:
  moqui load location=runtime/component/mr-mantle/data/MonitoringSalesSecurityData.xml
  ```

  The new dedicated security files avoid reloading unrelated tool/Mantle grants. Configure the report email template's SMTP server and delivery destinations for the installation; review existing custom template IDs before loading. No upgrade loads were run against the user's application database during acceptance.
- Legacy runtime hit recording and SearchFacade log shipping have been retired. `ArtifactHit`, `ArtifactHitBin` and their report view are no longer entity definitions. Visit/Visitor records remain. The former summary, bins, Log Viewer and DataDocument Search screens and hit REST resources are replaced by Monitor and Dashboards/Explore.
- Startup retires `clean_ArtifactData_daily`, `clean_ElasticSearchLogMessages_daily` and `clean_ElasticSearchHttpLogMessages_daily` independently. Their parameters are removed; registrations referenced by historical runs are paused with no cron expression, preserving those runs. Unreferenced registrations may be deleted. New telemetry retention and rollup jobs replace them.
- No historical hit/log data is migrated, and no historical table or index is automatically dropped. After backup and an administrator's explicit retention decision, default PostgreSQL deployments may run `DROP TABLE IF EXISTS public.artifact_hit;` and `DROP TABLE IF EXISTS public.artifact_hit_bin;`. Adjust schema/table names to the installation; these statements are optional and are never executed by startup.
- Telemetry mapping signature changes fail closed before rebuilding an existing telemetry index/table. Back up and explicitly migrate or create a fresh prefixed telemetry store before changing incompatible mappings. Business DataDocument rebuild semantics remain unchanged.
- OpenSearch telemetry ingestion acknowledges durable bulk indexing before the configured automatic search refresh; flush/shutdown guarantees backend acknowledgement rather than immediate search visibility. Ordinary SearchFacade writes and report/rollup writes keep their existing refresh behavior. PostgreSQL retains committed visibility.
- The server's SDK span/log processors try to enqueue owned records into the factory's bounded worker; normalization, transport and persistence stay off business threads. This avoids an unobservable SDK export queue. Core limits are 8,192 records and 32 MiB, reserving 512 records and 2 MiB within those limits for local request/job roots; external records and ordinary telemetry cannot consume that reserve. Large tail promotions admit roots before excess children without reordering tail decisions. Flush capacity acquisition and acknowledgement share a bounded five-second deadline. Core queues have bounded tail evidence and transport batches of at most 512. Queue, tail, persistence and remote losses are counted. Shutdown drains before SearchFacade closes, within bounded transport/shutdown deadlines.
- Ingest tokens are separate from login keys. They cannot sign in. Revocation, expiry, enabled owner and current ingestion permission are checked on each request; source identity comes from the verified key. Literal webhook credentials are rejected: dedicated server properties supply secrets. Shared legacy definitions redact credentials.
- Dataset context values come from authenticated identity (`userId`, `username`, `userGroupIds`, nonsecret `userAccount` fields). Organization-specific policy services receive the authenticated user ID and must return a filter. Saved definitions, exports, embeds and scheduled owner execution use the same authorized executor.
- Saved report definitions accept `orientation: "portrait"|"landscape"`, `layout: "grid"|"rows"`, and `appendix: true` in their body. Each dashboard selection carries its own variables and range. Offline print uses the same browser panel renderer, formatting and Chart.js; all row/tab sections are included in print.
- Deprecated `artifact-stats` configuration still applies with warnings, including bin-only slow evidence after warmup when hit persistence was disabled. Explicit new telemetry settings take precedence. `track-artifact-hit` descendant suppression remains effective; explicit span disabling suppresses artifact spans without dropping mandatory HTTP request history.
- Cumulative integer Sum snapshots retain exact signed integer state; integer intervals are subtracted before floating-point frame encoding. A switch between integer and double snapshots outside the exact double range establishes an explicit baseline and partial-success notice.
- Cumulative points with unknown/zero-duration starts and unannounced same-start resets establish a visible baseline rather than contributing an overlapping interval. New exponential histogram points are normalized to delta populations before minute/hour/day aggregation; old raw cumulative exponential records cannot be losslessly reinterpreted and remain retained as source evidence.
