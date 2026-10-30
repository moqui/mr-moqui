# Monitoring

M1–M7 were built Oct 26–27, 2026 (commits 735b3860 and 0df8ad3f). A review pass on Oct 27 fixed the bugs it found and listed the gaps it left in [Remaining-Work](Remaining-Work.md#monitoring); a second pass on Oct 27–28 built them (Prometheus text, protobuf export, outbound trace coverage, shared alert items, per-table exports, the report engine and dashboard gaps, the TUI charts, the Monitor screens and access), leaving the overhead measurements and smaller follow-ups there. A third pass on Oct 28 built the Dashboards later items (repeats; funnel, node graph, flame graph and geomap; log patterns; the agent-traces view; Git sync) and pulling (scrape targets and synthetic checks), and closed those follow-ups: received metrics on `/metrics`, SQL spans following their frame's sampling, gzip and merged OTLP export, shared silences matched by label, definition delete, notifications not sent twice, the TUI time picker, panel menu, suggestions and kiosk scrolling, data links that keep the time range, a signed-in `report-embed` test, a shipped latency heatmap, inline search highlights, Dashboards list links that work under BOSS Insights, decimals in TUI props, localization of the widgets and shipped dashboards, `unitField` amounts, and lookup labels without entity grants. The overhead measurements remain. The Later stages are not started. The user guide is [docs/tools/Monitoring.md](../docs/tools/Monitoring.md). A new Monitor app in the tools component, built on OpenTelemetry, replaces ArtifactHit/ArtifactHitBin and the `moqui_logs`/`moqui_http_log` shipping, and takes in OTLP from external systems. Its charts, dashboards, alerts and scheduled reports come from a general reporting tool on the SearchFacade, with its own Dashboards app, that serves business reporting as well ([Dashboards and reports](#dashboards-and-reports)).

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
- **Ingest:** OTLP over HTTP first, with gRPC deferred (built Oct 30: tonic on 4317, behind the `grpc` feature).
- **Dashboards and reports:** a framework engine (datasets, report queries, frames, panels, dashboards, alerts) with its own Dashboards app in the tools component. Monitor is its first user. Business reporting, app landing pages and BOSS Insights use the same pieces.
- **Definitions:** dashboards, panels, datasets and rules are saved as JSON in entities, with versions and named references. Components ship them as JSON files, which are read-only until saved as a copy.
- **Order:** the report tool comes early (M2 and M3), so the Monitor screens are built from panels from the start.
- **References:**
  - SkyWalking (Apache-2.0) and SigNoz (MIT, apart from its proprietary `ee/` tree), for telemetry.
  - For dashboards and reports:
    - Grafana (AGPL-3.0, apart from the Apache-2.0 `packages/grafana-data`, `grafana-ui`, `grafana-runtime` and `kinds/` trees; its `apps/dashboard/kinds` schema is AGPL).
    - OpenSearch Dashboards (Apache-2.0; its reporting, alerting and observability plugins are separate repositories, not in the clone).
  - All are cloned under `~/work/moquiwork/3p_other/`. We borrow ideas from them, not code or schema text, and never read `ee/`.

### Starting point (Oct 25, before M1)

This is the code as it stood when the plan was written. M1 and M2 replaced the logging, spans and hits described here.

- **Logging.** [`LineSubscriber`](../crates/moqui-host/src/logging.rs) is a hand-written `tracing` subscriber.
  - It hands out span ids but keeps no span data.
  - Events go to stderr, the TUI and SSH taps, and rolling files.
  - `ship_to_search` also sends them every 2 s to `moqui_logs`, and the `log_http` middleware ([`http.rs`](../crates/moqui-host/src/http.rs)) sends to `moqui_http_log`. Both go through `SearchFacade::index_log_entries`.
  - There is no `tracing-subscriber`, and no OpenTelemetry, prost or tonic crate in `Cargo.lock`.
- **Spans.** There are two `tracing` spans:
  - `artifact`, opened at each artifact-stack push and closed at pop ([`context/artifact.rs`](../crates/moqui-core/src/context/artifact.rs)). Screens, transitions, services, entities, REST and LLM calls all push frames.
  - `sql`, in [`entity/exec.rs`](../crates/moqui-entity/src/entity/exec.rs).
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
| `opentelemetry-otlp` (http-proto) | Not used. The optional external exporter is our own: it posts OTLP protobuf (or JSON, by `otlp-protocol`) through `moqui_lib::http::RestClient` (`telemetry/otlp.rs`), encoded with the `opentelemetry-proto` messages the receiver uses |

- **Versions.** All `opentelemetry*` crates go on one minor version, pinned by `Cargo.lock`.
- **Not used:** `tracing-subscriber`, `tracing-opentelemetry` and `opentelemetry-appender-tracing`. Spans come from the artifact stack and logs from `LineSubscriber`, so neither needs a `Registry`. tonic is only the gRPC receiver's server, under moqui-host's `grpc` feature.
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
  - Each LLM provider call gets a CLIENT span under its `AT_LLM` frame, with the OTel `gen_ai.*` attributes: provider, model, input and output tokens, and finish reason. AI telemetry (requirement 2) then needs no separate store. The agent-traces view (Monitor › AI, built Oct 28) groups these calls by trace and names each by its AT_LLM frame.
- **SQL:** a CLIENT span per statement inside a trace, with `db.system.name` and the parameterized `db.query.text`. A statement outside any trace (a scheduler poll, a data load) gets no span, but its duration metric is still recorded. The slow-statement warning (now with the SQL redacted) and the statement counters stay.
- **Jobs:** a root span per `ServiceJobRun`, with the trace id saved on the run so the job screens can link to it. Its error is the job's first error message.
- **Not recorded:** `/health`, `/metrics` and the OTLP receiver paths write no request span.
- **Errors:** a frame is marked ERROR only when an error was added while it was on the stack, so its parents carry it but siblings that run after it do not. A 5xx request root keeps the real error message.
- **Propagation.** W3C `traceparent`/`tracestate` is:
  - read on requests
  - carried in `CallJob` next to `origin`, so async services and pool work join the trace
  - injected by `outbound.rs` and remote services
  - not carried from the qvt page. The page sends its own trace id as `X-Moqui-Page-Trace` on each fetch, and the request root records it as `moqui.page.trace_id`; each request is its own trace. (Carrying the page's `traceparent`, as first built, made a whole browser session one trace.) Browser spans come later with RUM
- **Sampling.** Head sampling is set in config: `sample-ratio`, 0 by default, decided from the trace id so services with the same ratio agree. An incoming `traceparent` keeps its trace and parent ids, but its sampled flag is not trusted. The local exporter keeps every trace that has an error or a slow root (a simple tail rule) and drops the rest of the unsampled traces.
  - Every request's root span is kept whatever the sampling, as `moqui_http_log` rows and screen ArtifactHits were written for every request. This keeps a visit's request history whole. Sampling and the tail rule apply to the spans below the root, and an unsampled root is marked so.
  - The tail rule holds each unsampled trace's spans in a bounded buffer until the root ends. The buffer caps the number of traces and the spans per trace; past the cap, the head decision stands.
  - A short-lived decision cache by trace id covers spans that end after their root, such as `CallJob` work.

**Metrics** (SDK instruments, with OTel semantic names where they exist)
- **Instruments:**
  - `http.server.request.duration`
  - `moqui.artifact.duration`, labelled by type and name (and action where there is one). It replaces ArtifactHitBin: artifact names are a bounded set, and the bin becomes one minute.
  - `db.client.operation.duration`
  - `moqui.job.runs`, counted by outcome (success, error, panic)
  - observable gauges for pool, request queue, guards and process, from the values `/metrics` and `status_map` already compute
- **Export:** the periodic reader runs every 60 s and writes one document per series per interval (count, sum, min, max and histogram buckets). This is SkyWalking's pre-aggregation done by the SDK, so RED charts never scan raw spans.
  - The reader uses delta temporality, so each document holds one interval's values, and a series with no activity in an interval writes nothing.
  - Histogram boundaries are fixed per instrument (the semantic-convention boundaries for durations), so documents of one series can be merged.
- **Volume:** there is at most one minute document per active series, which is series × instances × 1440 a day. For example, 2,000 artifact series on one instance give about 2.9 M documents a day; this has not been measured at production cardinality. An attribute allow-list keeps the series bounded: a global list, replaced per instrument by `<telemetry><metric name attributes>`.
- **Rollups:** a job rolls minute documents up to hour and day documents.
- **`/metrics`** stays, with the telemetry counters added, and answers Prometheus text (`?format=prometheus` or a Prometheus `Accept`) from a second, cumulative SDK reader (`telemetry/prometheus.rs`). Guard values are observable gauges.

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

- **Typed columns.** `bounds`, `buckets` and `sample_trace_ids` stay in the body; the rest of the key fields above are typed.
- **Time.** OpenSearch `date` holds milliseconds, and the facade has no nanosecond field kind. So spans keep `start` for ranges and histograms, and `start_nano` (a long) for ordering and the waterfall.
- **Errors.** The local exporter upserts `otel_errors` when a span ends in error or an exception event is recorded. This is the SigNoz groupID idea.
  - One failing request counts once per group, however many of its spans carry the error.
  - The count is an atomic increment, so instances do not overwrite each other: `ON CONFLICT … DO UPDATE` on Postgres, and a scripted upsert on OpenSearch. This is a new facade capability.
- **Annotations** are requirement 1's change records. They hold:
  - deploy, config and release markers
  - alert state changes
  - notes added by hand on a chart

  CI posts deploy markers to `POST /v1/annotations` with an ingest key; signed-in users and scripts can use `monitor/annotations` in `moqui.rest.xml`. Time series panels draw annotations as markers and bands.
- **Retention.** A daily ServiceJob replaces `clean#ArtifactData`. It deletes by time with `delete_matching`, with separate settings per index and per metric granularity: raw spans and logs for days, minute metrics for days, hour and day metrics for months, and annotations for years.
- **Day partitions** (built Oct 30): a Postgres-backend option, `<search-facade><postgres partition-telemetry="true">`, partitions these tables by day, so retention drops partitions instead of deleting rows (SkyWalking's day tables). `otel_metrics` is split by granularity first, as each has its own retention; `otel_errors` is upserted across days and is not split. The key becomes `(doc_id, [granularity,] time)`, kept out of the registry signature so an existing table is not refused as a changed mapping; it stays unpartitioned with a warning until `partition#TelemetryTables` (Monitor › Sources) copies it a day at a time and swaps it in under a short lock, keeping the old table to drop (`search/postgres/partitions.rs`, test `tests/it/telemetry_partitions.rs`).
- **Telemetry config.** It replaces `artifact-stats`. The settings: per-type enable and sampling (keep the artifact-type list), retention, the exporters, and the receiver. `stats-skip-condition` goes away: it is not read, the XSD marks it deprecated, and a conf that sets it gets a startup warning. An `artifact-stats` left in a site or component conf still applies, with a deprecation warning ([Retired and replaced](#retired-and-replaced)).

**SearchFacade additions** for the telemetry screens:
- `Avg` (or sum and count read together)
- `Percentiles`: `percentile_cont` on Postgres, native on OpenSearch
- a histogram-bucket merge for metric documents, so p50/p95/p99 can come from the SDK buckets without reading raw spans. It merges only series with the same boundaries.

These go into `Agg` and both backends, with tests on each. The report tool needs more; see [Report queries](#report-queries).

### Ingest

- **Endpoints:** `POST /v1/traces`, `/v1/logs` and `/v1/metrics` on the existing axum server, taking `application/x-protobuf` and `application/json`. Responses follow OTLP, including partial success.
- **Auth:** an API key tied to a user with a `MONITOR_INGEST` permission. Each key names the source, and the source is recorded on what it sends.
- **Limits:** body size, rate per key, and a bounded queue that rejects when full and counts what it rejects. Redaction rules run before indexing. One rule for sensitive names (`context/identity.rs`) covers parameters, URL queries and span attributes.
- **Decoding:** `opentelemetry-proto` decodes into the same documents the local exporter writes, so every screen works on external data unchanged.
- **Temporality:** many senders, Prometheus-style ones among them, send cumulative points. These become deltas through a bounded per-series cache of the last point, keyed by series fingerprint and start time; a lower value or a new start time is a reset. Exponential histograms became deltas the same way in the completion wave, and are merged for the rollups by scaling down to common buckets (`telemetry/exponential.rs`).
- **gRPC** (built Oct 30): tonic on 4317 behind the moqui-host `grpc` feature (in Enterprise and Maximal), opened by `<telemetry><receiver grpc-enabled="true">`. The collector services share the HTTP receiver's key check, rate limit, concurrency budget, decoder and queue (`monitor/receiver.rs`); the request message reaches the decoder through its serde form (`Decoder::decode_value`), as a protobuf body does. TLS uses the HTTPS listener's certificates with ALPN h2; refusals are gRPC statuses (`monitor/grpc.rs`, test `tests/it/monitor_grpc.rs`).
- **Pulling** (built Oct 28): ServiceJobs scrape Prometheus-text endpoints (`moqui.monitor.ScrapeTarget`) into `otel_metrics` through the same decoder, so cumulative series become deltas, and run synthetic HTTP checks (`moqui.monitor.SyntheticCheck`), each recorded as a kept, fully sampled root span with its request as a CLIENT child (`telemetry/scrape.rs`, Monitor › Checks).

### Failure tolerance

- Telemetry never blocks or fails business work. Exports are bounded, run off the request thread, and never write inside a business transaction.
- A failed batch, a document that will not index, or a bad OTLP record is logged once (rate-limited), counted, and skipped. Documents the store did not persist count as `failed` (a subset of `dropped`), with one log line per batch; a panicking exporter costs only its batch. An ingest request refused before decoding (busy, rate limit, size, bad body) is answered with an error, not counted as dropped records.
- When the search backend is down, telemetry is dropped and loss counters go up. Those counters show on `/metrics` and on the Monitor Sources screen.
- Missing telemetry shows as missing, never as healthy (requirement 2).

### Monitor app

`runtime/base-component/tools/screen/Monitor.xml` is added to `apps.xml` from `tools/MoquiConf.xml` at menu-index 97 (System is 98, Tools is 99). It uses icon `sym_o_monitoring` and a `MONITOR_APP` artifact group in `MonitoringSecurityData.xml`.

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
  - polling for the log tail (as built: the Explore `tail` transition every 2 s; a `/notws` topic was the first idea)
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
  - Entities: ArtifactHit, ArtifactHitBin and the ArtifactHitReport view (`ServerEntities.xml`), their fence rows ([`entity/fence.rs`](../crates/moqui-entity/src/entity/fence.rs)), and the nontransactional-group note in `MoquiDefaultConf.xml`.
  - Recording: [`context/hits.rs`](../crates/moqui-core/src/context/hits.rs), `count_artifact_hit`, and the bin flush at shutdown in `moqui-host`.
  - Screens and REST: the System `ArtifactHitSummary.xml` and `ArtifactHitBins.xml` screens, and their REST list entries in `tools/service/moqui.rest.xml`.
  - Cleanup: `clean#ArtifactData` and its `clean_ArtifactData_daily` job, replaced by the telemetry retention job.
- **`artifact-stats`.** The defaults move to the telemetry config. An `artifact-stats` left in a site or component conf still applies, with a deprecation warning, the way schema problems stay warnings: `persist-bin` turns the type's metric on or off, and `persist-hit` its spans.
- **`track-artifact-hit="false"`** stays on the screens that set it (`webroot` `sso.xml`, `emailOAuth.xml`, `echopath.xml`, `rest.xml`), as does the per-frame flag in `screen/render.rs`. It now means no artifact spans or artifact metrics under that screen. The request span is still written.
- **Untracked frames.** A frame the code untracks (a REST path frame, the screens around always- and pre-actions, an authorization check) records no span or metric of its own, but the frames under it are recorded. As first built, untracking hid the whole subtree, so services called over REST had no spans.
- **Slow frames.** The slow rule moves into the telemetry module, reusing the per-artifact running mean and variance in `hits.rs`: a frame is slow at more than 2.6 standard deviations above its artifact's average, after 50 frames.
  - A slow frame gets `moqui.artifact.slow` on its span (only when true) and counts in a `moqui.artifact.slow` counter.
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
| `DataSnapshot.xml` comment; the openui skill (`openui-lang.md`, `llm/ui/openui_tests.rs`); the Assist agent spec and two replays; `WebrootAssistCanvas.page.js` | Use ArtifactHitSummary as an example path | Use a Monitor screen, or Visit/VisitList where a list is needed |
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

- **Both backends.** The comments in `logs.rs` and `LogViewer.xml` say log indexes exist only on OpenSearch. That is stale: Postgres keeps them too (`keeps_log_indexes`, `crates/moqui-host/tests/it/search_logs_postgres.rs`). `otel_logs` is on both backends as well.

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
  - Relative ranges use Grafana's syntax (`now-6h`, `now/d`, `now-1M/M`, `now/w`, `now/fy`). They are parsed in Rust, with the week start and fiscal year start from `<report week-start fiscal-start-month fiscal-start-day>` in the conf. DateRange bounds such as `now-30d` are resolved on the server for both backends, rounding `lte` and `gt` bounds up as Elastic date math does.
  - The auto interval aims at a target bar count, capped by a maximum and rounded to a fixed table (OSD's approach). It says when it has scaled a requested interval up.
  - The bucket still filling is dropped or drawn as partial, so a rate chart does not show a false fall. Dropping it is a warning, not missing data, so alerts and scheduled reports still use the result.
- **Filters:**
  - Pills can be pinned, negated, disabled, edited as a query, and labelled.
  - Clicking a bar, slice or cell turns its bucket into a pill: Other becomes a negated list, and Missing becomes "does not exist".
  - Brushing a time chart sets the range.
- **Service source.** A service can return rows or a frame instead, for reports the facade cannot compute, such as the `LedgerReportServices` statements or budget against actual. A panel can mix queries of both kinds.

**SearchFacade additions** for the report tool. They come with Avg, Percentiles and the bucket merge from [Indexes](#indexes), go into `Agg` and both backends, and have tests on each:
- **DateHistogram:** fixed intervals (`10s`, `5m`, `7d`), extended bounds so the whole range is filled, and dropping partial buckets.
- **Terms:** Missing and Other buckets, and `min_doc_count` (at least 1 for terms, so empty terms from rows a filter hides never show). Both backends compute Other with a second query, as OSD does.
- **Metrics:** ValueCount, Stats, Median, and TopHits. TopHits gives sample documents per bucket, such as the trace ids behind a latency bar.
- **Buckets:** Histogram, Range and DateRange (for aging buckets), and named Filters.
- **Across types:** aggregations over all the types of an index, for index-wide datasets. Built as one query over every type (a multi-index request on OpenSearch, a `UNION ALL` on Postgres), so totals, averages and percentiles are exact.
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
- **Visualizations, second set** (built Oct 28): node graph (service map), flame graph (span self time; no profiles are stored), funnel, geomap.
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
  - Repeating a panel or row for each value of a variable (built Oct 28), expanded on the server.
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
  - **Keys:** `t-` zoom out, `t←`/`t→` shift the range, `e` edit, `v` view, `i` inspect, `d k` kiosk, Esc back out of kiosk and then the single-panel view. The TUI uses the same keys, kiosk included.
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
  - Each series or row is its own alert instance. A rule tracks at most 500 (more give a `series_limit` notice), and a series missing for `pruneAfterMissing` evaluations in a row (10 by default) is removed, with a resolved notification if it was not Normal.
- **Evaluation:**
  - A ServiceJob runs the rules each minute, grouped by interval.
  - Normal goes to Pending, then to Firing after the pending period. Firing goes to Recovering, then to Normal after keep-firing-for.
  - No Data and Error are states of their own and never count as healthy (requirement 2).
- **Routing:**
  - Labels from the rule and the series route an alert to contact points: NotificationMessage topics (the header notifications), email through `EmailTemplate`, and a webhook.
  - Notifications are grouped by labels, with group-wait and repeat intervals.
  - Silences (one-off, by labels) and mute timings (recurring, such as weekends) hold notifications back. As built, they live in the rule's body or as shared `contactPoint`, `silence` and `muteTiming` definitions that rules reference by id.
- **History:** state changes are written to the annotations index, so they show on dashboards. Each rule links back to its panel.
- **Business rules** use the same model, for example daily sales below a floor or overdue receivables above a ceiling.

### Scheduled reports and sharing

- **Reports** extend `ScreenScheduled`, or use a sibling entity if its fields do not fit. `ScreenScheduled` already gives a cron schedule, email to a user or group through `EmailTemplate`, saving to a location, and skipping when empty. A report adds:
  - one or more dashboards, each with its own time range and variable values
  - `paper` (letter or a4)
  - PDF, portrait or landscape, in the dashboard's grid or one panel per row, with an optional appendix of table data
  - CSV and xlsx, one file or sheet per table panel (a dashboard with no table panel exports each panel with data)
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
- **As built:** the `moqui.report` package (Definition, DefinitionVersion, DefinitionReference, AlertInstance, AlertRuleLock, AlertNotification) and `moqui.monitor.IngestKey`. Components ship definitions in `report/`, one JSON file each with `id`, `type`, `title` and `body`. Shipped definitions stay in memory, read-only. A shipped alert rule is a template: it runs only once saved as a copy, which gives it an owner.

### Security and cost

- **Every query runs as the viewer,** through the dataset's permission and row filters.
  - A saved panel never widens access.
  - A report runs as its owner, and the owner's access is checked at each run.
- **Limits:** row, bucket and time limits on every query. Exports are audited: definition export, report and panel downloads, Explore exports (`audit#Export`) and scheduled deliveries log to the `moqui.report.audit` target, which lands in `otel_logs`.
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
  - Postgres-backend index and search (like `crates/moqui-host/tests/it/search_logs_postgres.rs`)
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
- **Partitions and gRPC.** Day partitions on the Postgres backend, and the gRPC receiver (both built Oct 30).
- **Dashboards, later:**
  - repeating rows and panels (built Oct 28)
  - node graph, flame graph, funnel and geomap (built Oct 28)
  - grouping log lines into patterns (built Oct 28: `report/patterns.rs`, Explore's log patterns)
  - an agent-traces view over `gen_ai.*` (built Oct 28: Monitor › AI)
  - Git sync of definitions (built Oct 28: two-way, per folder)
  - public links, if they are wanted

Completion of the later stages is gated on: correlated failures, tenant and privacy isolation, tolerance of a telemetry outage, measured overhead, regression detection, and safe, retried and failed recovery tests. Other signal types and specialized infrastructure wait until they are needed. No extra server is required by default, and an OpenTelemetry Collector stays optional.


## Implementation verification log

M1–M7 acceptance ran Oct 26–27, 2026 on one macOS host (Apple M1 Pro, 8 CPUs, 32 GiB RAM; rustc 1.99.1, `develop` profile). This is a summary; the full log, with per-run tables and the diagnostic history, is in commit 0df8ad3f. Overhead was measured on the first-pass build and on the final build only. Per-milestone baselines were not captured, so the incremental cost of each milestone is not known.

### Functional acceptance

| Milestone | What passed |
| --- | --- |
| M1 | Span trees for screens, services and entities; propagation through the worker queue, jobs and streamed HTTP; every request root kept with child sampling at zero; error and slow promotion; queue saturation and exporter failures kept away from requests; browser and TUI trace waterfalls, and job links |
| M2 | The replacement readers (Visit detail and list, Logs, Artifacts) on PostgreSQL and live OpenSearch, before the old readers were removed; minute metrics for a known workload; retry-safe hour and day rollups; every new aggregation on both stores; cursor paging over 10,025 rows with equal timestamps |
| M3 | Dataset, group and row restrictions applied before totals, paging, documents, embeds and exports on both stores; read-only shipped definitions, copies, versions, import and references; the Mantle sales dashboard; the browser editors, Explore and the inspector; every new screen in the TUI |
| M4 | Fingerprints stable across message parameters and line shifts; concurrent counts from two writers add up on both stores; errors link to their logs and traces |
| M5 | Ingest keys (revocation, owner, permission), limits, compression, redaction, partial success and cumulative resets; a real OpenTelemetry Collector sending JSON and protobuf for all three signals, and receiving all three from the exporter |
| M6 | Alert states under a controlled clock (Pending, Firing, Recovering, Normal, No Data, Error), hysteresis, grouping, repeat, silences and mute timings, restart, four concurrent evaluators, and SMTP and webhook delivery with retries. Version comparison is Explore's Split screen over two ranges or filters, on raw documents, with RED charts per `service.version` from the request metric |
| M7 | Chromium PDF and PNG with the network blocked, waiting for the charts; CSV and xlsx from the same authorized frames; empty reports skipped; schedule leases under restart and concurrency; absolute-time links, embeds and playlists |

Counts on Oct 27, before the review pass: core 2,021 passed (three ignored: a manual benchmark and two live OpenSearch tests, which passed in the 73-test search run), host 217, TUI 176, `monitor_edge` 6/6, search 73/73 against OpenSearch 3.0.0, replacement readers and rollups 5/5, server JS specs 29/29 and 1/1, browser specs `WebrootReport` 18/18, `WebrootAssistCanvas` 16/16 and `Monitoring` 6/6. `cargo lint` and `cargo lint-min` passed.

Commands, from the repository root, with embedded PostgreSQL and Chromium available:

```sh
cargo test -p moqui-core --features telemetry --lib --offline -- --test-threads=4
cargo test -p moqui-tui --offline
cargo test -p moqui-host --lib --offline -- --test-threads=1
cargo test -p moqui-host --test it --offline -- monitor_edge:: report_monitoring:: report_alerts:: report_schedule_lock:: telemetry_rollup:: monitor_readers:: monitor_tui:: search_logs_postgres:: report_documents:: mantle_search::
cargo test -p moqui-host --test tui_screens --offline gallery_screens -- --exact --nocapture
target/develop/moqui test framework --spec ScreenHtml,ScreenQvt --db none
target/develop/moqui test-client tools --serve --spec Monitoring
target/develop/moqui test-client webroot --offline --spec WebrootReport
cargo --offline lint
cargo --offline lint-min
```

Against disposable services (OpenSearch on 19200, a Collector on 14318; the tests use prefixed indexes and scratch databases, and the Collector-forwarding test starts its own container with `--pull=never`):

```sh
MONITOR_READER_OPENSEARCH_URL=http://127.0.0.1:19200 cargo test -p moqui-host --test it --offline -- monitor_readers:: telemetry_rollup:: report_documents:: --ignored --test-threads=1 --nocapture
MONITOR_OPENSEARCH_URL=http://127.0.0.1:19200 cargo test -p moqui-core --features telemetry --lib --offline search::opensearch::tests::monitoring_aggregations_and_cursor_on_live_opensearch -- --ignored --exact --nocapture
MONITOR_COLLECTOR_URL=http://127.0.0.1:14318 cargo test -p moqui-host --test it --offline -- monitor_edge:: --ignored --test-threads=1 --nocapture
```

### Overhead

The Profiling fixtures with the recording sink, telemetry off and then on at full sampling (median µs per call; service rows are 1,000 calls × 5 batches, the others 3 batches of 500 calls or 100 renders):

| Operation | Off → full, µs | Ratio | Allocations off → full |
| --- | ---: | ---: | ---: |
| service echo | 8.93 → 17.40 | 1.95× | 94 → 127 |
| uncached entity PK read | 17.77 → 38.98 | 2.19× | 38 → 56 |
| cached entity PK read | 1.65 → 1.72 | 1.04× | 19 → 19 |
| nested service | 11.89 → 27.04 | 2.27× | 97 → 166 |
| plain HTML label | 17.66 → 38.75 | 2.19× | 122 → 217 |
| form-single QVT, 11 fields | 436.13 → 471.46 | 1.08× | 1,494 → 1,627 |
| QVT list, 100 rows | 9,462.78 → 9,498.40 | 1.004× | 25,192 → 25,310 |

Each span has a fixed cost, so small operations pay the most and larger screens dilute it.

An authorized dashboard request over HTTP, with a row policy, 200 requests on one worker and then 200 on four (p50 / p95 ms):

| Store | Sampling | One worker | Four workers | Records dropped |
| --- | --- | ---: | ---: | ---: |
| PostgreSQL | off | 3.20 / 3.84 | 4.81 / 7.26 | 0 |
| PostgreSQL | roots | 3.70 / 4.05 | 5.62 / 6.71 | 0 |
| PostgreSQL | full | 3.86 / 4.53 | 5.60 / 7.06 | 4,984 |
| OpenSearch | off | 5.48 / 6.79 | 7.40 / 8.77 | 0 |
| OpenSearch | roots | 6.79 / 7.70 | 8.87 / 11.25 | 0 |
| OpenSearch | full | 7.14 / 10.00 | 9.83 / 15.82 | 0 |

- **Capacity.** Full sampling overloads the PostgreSQL store in this burst: 4,984 child records dropped (5,250 with the OTLP tee), while every request root is kept. Roots mode drops nothing on either store, which is why the default ratio is 0. The queue bounds are fixed: 8,192 records and 32 MiB, with 512 records and 2 MiB of them reserved for request and job roots. Losses are counted on `/metrics` and on the Sources screen.
- **OpenSearch writes.** Telemetry bulk writes don't wait for a refresh (other SearchFacade writes still do). Draining 1,248 records went from 3.89 s to 15 ms.
- **Outbound OTLP** to a local Collector: a 512-record batch takes 6–9 ms at p50.
- **Not measured:** production concurrency and cardinality, long-running retention and rollup load, storage growth, WAN or TLS transport to a Collector, a multi-host soak, and QuickJS C allocations.

To reproduce, build `monitor_performance`, `service_speed` and `js_speed` with `--profile develop --no-run` and run them serially with `--ignored`, setting `telemetry_enabled`, `telemetry_sample_ratio`, `MONITOR_PERF_MODE` (`off`, `roots` or `full`), `MONITOR_PERF_OPENSEARCH_URL` and `MONITOR_PERF_COLLECTOR_URL`. The Collector config is `crates/moqui-host/tests/fixtures/monitoring/collector-performance.yaml`.

### Review pass (Oct 27)

A review of the plan against the code found bugs the acceptance runs had missed: the tests rendered screens without following their links, worked around a partial-bucket notice, and loaded seed files by hand. Fixed:

- **Telemetry.**
  - Untracking a frame hid its whole subtree, so services called over REST or from always- and pre-actions had no spans or metrics.
  - The qvt page sent its own `traceparent` on every fetch, which made a browser session one trace, and a client's sampled flag decided sampling.
  - The tail rule dropped late error spans, and cleared its decision cache when full.
  - A store outage logged a line per document and never counted as failed.
  - A 5xx root lost its real error, and `otel_errors` counted every span of a failing request.
  - Every background SQL statement was its own trace, `/health` and `/metrics` wrote request roots, reserved-thread closures lost the trace, and `primaryKeyId` was redacted from URLs.
- **Reports.**
  - A dropped partial bucket marked the result incomplete, so histogram alerts sat in Error and scheduled reports of the shipped dashboards were refused.
  - The `ReportAlert` topic delivered to nobody who had not opted in.
  - On OpenSearch, terms with `min_doc_count:0` listed values outside the row filter.
  - Malformed query JSON panicked, and one bad panel or variable failed the whole dashboard.
  - The single-panel view showed placeholders, and OpenSearch p99.95 and Postgres relative date ranges failed.
- **Screens.**
  - Links and forms on the Monitor screens built on Dashboards screens went to missing URLs.
  - A new ingest key's token was lost in the browser.
  - Job run log and trace links failed.
  - Playlists left the shell and showed tables.
  - The time form dropped variables.
  - Frozen cells were unreadable in light mode.
  - Visit detail ignored its filters and missed a visit's first request.
  - Decimal values reached the dashboard widgets as `{}`.
- **Seed data.** `TelemetryData.xml`, `ReportSetupData.xml` and `ReportEmailData.xml` were not in the conf's `load-data`, so a new database had no retention, rollup or alert jobs.
- **Default.** The sample ratio is now 0 (roots plus the tail rule).

The cheap improvements in the same pass:
- **Hot path:** fewer allocations and locks per span, and no SQL spans outside a trace.
- **Shipped dashboards:** automatic intervals, top error groups and a loss stat on Overview.
- **Monitor screens:** requests by default on Traces, readable error links, and Logs links from traces and errors.
- **Other:** dataset listing, app icons, confirmations, aria labels, Esc out of kiosk, export auditing, alert pruning and a series cap, a `paper` option, and `<report>` in the default conf.

Hot-path cost before and after, on Linux with the `develop` profile (median µs per call; the machine was shared with other builds, so read the allocation counts as the steady measure):

| Call | Telemetry | Before → after, µs | Allocations before → after |
| --- | --- | ---: | ---: |
| service echo | off | 9.91 → 9.77 | 94 → 90 |
| service echo | ratio 0 | 16.78 → 14.31 | 127 → 115 |
| service echo | ratio 1 | 17.53 → 14.16 | 127 → 115 |
| uncached entity PK read | ratio 0 | 27.29 → 21.13 | 55 → 39 |
| noop inline JS service | ratio 0 | 11.95 → 7.91 | |
| nested service | ratio 0 | 27.00 → 21.27 | |

The entity read is called outside any trace in this fixture, so it no longer makes a SQL span; inside a request it still does. Queued records per service run fell from 110,919 to 70,559 for the same reason.

Counts after the review pass, on Linux: core 2,063 passed (three ignored, as before), host 225, TUI 181, every file of the host command above, the `it` tools tests 3 (run with `--test-threads=1`: an MCP test the filter also matches takes the same SSH port) and the slow `tools_screens` test, the gallery snapshots, server JS specs 29/29, `Monitoring` 12/12 and `WebrootReport` 33/33. `cargo lint` and `cargo lint-min` passed. The live OpenSearch and Collector tests were not rerun.

`monitor_tui` failed about one run in five, before this pass too: with focus on the live tail, the tail's lines fell below the 100-row screen whenever the Explore histogram took an extra row. It now draws 160 rows and passed 12 runs in a row.

Upgrade notes and deployment defaults are in [docs/tools/Monitoring.md](../docs/tools/Monitoring.md#upgrading).
