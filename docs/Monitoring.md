# Monitoring, dashboards and reports

Moqui records its own work as OpenTelemetry spans, metrics and logs, takes in OTLP from other systems, and keeps all of
it in SearchFacade indexes on Postgres or OpenSearch. The Monitor app reads those indexes. The Dashboards app is a
general reporting tool over the same indexes and over business DataDocuments: datasets, report queries, panels,
dashboards, alerts and scheduled reports. The design and the build record are in
[plans/Monitoring.md](../plans/Monitoring.md). As of Oct 27, 2026 (M1–M7 and the review pass built).

The code: `crates/moqui-core/src/telemetry/` (the SDK wiring, sampling, the bounded worker, OTLP in and out, rollups
and retention), `crates/moqui-core/src/report/` (datasets, queries, frames, transformations, definitions, alerts,
annotations), `crates/moqui-core/src/report_schedule.rs`, `crates/moqui-core/src/search/telemetry.rs` (the index
mappings), and the host side in `crates/moqui-host/src/http.rs` (request spans) and `crates/moqui-host/src/monitor.rs`
(the OTLP receiver). Screens: `runtime/base-component/tools/screen/Monitor/` and `Dashboards/`; widgets in
`webroot/screen/webroot/js/WebrootReport.js` with TUI twins in `crates/moqui-tui/src/components/report.rs`.

## What is recorded

- **Request spans.** Every HTTP request gets a SERVER root span, whatever the sampling: route, status, user, visitor,
  visit, client address, redacted query, sizes and time to first byte. The Visit screens and the Monitor Traces screen
  read these. Each request is its own trace; a qvt page sends its own trace id as `X-Moqui-Page-Trace` and the root
  records it as `moqui.page.trace_id`.
- **Artifact spans.** A span per screen, transition, service, REST path, LLM call and (off by default) entity frame,
  with `moqui.artifact.type`, `.name`, `.action` and `.sub_type`. A frame that runs well above its artifact's average
  (2.6 standard deviations, after 50 runs) is marked `moqui.artifact.slow`.
- **SQL spans.** A CLIENT span per statement inside a trace, with `db.system.name` and the parameterized
  `db.query.text`. Statements outside any trace (scheduler polls, data loads) get no span.
- **Job spans.** A root span per ServiceJobRun; the run keeps its trace id, so the job run screen links to the trace.
- **Metrics.** `http.server.request.duration`, `moqui.artifact.duration` (by type, name and action),
  `db.client.operation.duration`, `moqui.job.runs` (by outcome) and process gauges, written once a minute as one
  document per active series, then rolled up to hours and days.
- **Logs.** Every INFO and higher log line, with the trace and span ids of the frame that wrote it.
- **Errors.** Each error span or exception is fingerprinted (type, normalized message, top frames) into `otel_errors`,
  counted once per failing request.

The indexes are `otel_spans`, `otel_logs`, `otel_metrics`, `otel_errors` and `monitor_annotations`.

## Turning it on and sampling

Telemetry is on by default in builds with the `telemetry` feature (`lite` and `enterprise` have it). The default conf,
with an `artifact` override and the receiver limits written out:

```xml
<telemetry enabled="${telemetry_enabled}" sample-ratio="${telemetry_sample_ratio}" interval-seconds="60"
        span-retention-days="7" log-retention-days="7" error-retention-days="90"
        minute-retention-days="7" hour-retention-days="90" day-retention-days="730" annotation-retention-days="730">
    <artifact type="AT_ENTITY" spans="true" metrics="true"/>
    <artifact type="AT_SERVICE" sample-ratio="0.25"/>
    <metric name="http.server.request.duration" attributes="http.route,http.request.method,http.response.status_code"/>
    <receiver enabled="true" body-limit="4194304" requests-per-minute="600" concurrent-requests="8"/>
</telemetry>
```

- **`telemetry_enabled=false`** turns it all off.
- **`sample-ratio`** (`telemetry_sample_ratio`, default `0`) is the share of traces whose child spans are all kept.
  At `0` (roots mode) every request and job root is kept, and the child spans of a trace are kept only when the trace
  has an error or a slow root (over 1 s) or slow frame; the rest are dropped. At `1` every span is kept. The decision
  is made from the trace id, so services with the same ratio agree. The sampled flag of an incoming `traceparent` is
  not trusted.
- **Cost.** With telemetry on, a small service call costs about half as much again as with it off (about 10 → 14 µs
  after the review pass), and at full sampling a burst of requests can fill the queue on a Postgres store; larger
  screens dilute the fixed cost. Roots mode dropped nothing in the same burst. Raise the ratio for a development server, or for a while when you need full
  traces.
- **`artifact`** turns spans and metrics on or off per artifact type. Entity frames are off by default and screen
  content gets metrics only; the dev conf (`runtime/conf/MoquiDevConf.xml`) adds content spans and entity metrics.
  Its `sample-ratio` (0 to 1) overrides the global ratio for that type's frame spans (and `AT_LLM` provider spans);
  roots and the tail rule are unchanged, and SQL and other child spans follow the global ratio.
- **`metric`** sets the attributes kept on one instrument, replacing the global allow-list for it; `attributes=""`
  keeps none. Instruments without one keep the global list.
- **Capacity:** `queue-records` (8192), `queue-bytes` (33554432), `tail-traces` (1024) and `tail-trace-spans` (512)
  on `<telemetry>`. A sixteenth of the queue is kept for request and job roots. A bad value warns and keeps the default.
- **Retention** is per index and per metric granularity, in days; the `clean_TelemetryData_daily` job applies it and
  `rollup_TelemetryMetrics_hourly` rolls minutes into hours and days.
- **`interval-seconds`** is for tests; the metric documents are minute documents.
- A screen with `track-artifact-hit="false"` records no artifact spans or metrics for itself and anything under it.
  Its request span is still written.
- `server-stats/artifact-stats` from older confs still applies, with a deprecation warning: `persist-hit` turns the
  type's spans on or off and `persist-bin` its metric. An explicit `telemetry/artifact` wins.

## OTLP in and out

**In.** `POST /v1/traces`, `/v1/logs` and `/v1/metrics` take OTLP/HTTP as `application/x-protobuf` or
`application/json`, gzip allowed, and answer as OTLP does, with partial success.

- Each sender needs an ingest key: Monitor › Sources › Create key, which shows the token once. Send it as
  `Authorization: Bearer <token>` or an `api_key` header. A key names its source, which is recorded on everything it
  sends, and belongs to a user who must keep the `MONITOR_INGEST` permission. Keys expire (90 days by default) and
  can be revoked. They cannot sign in.
- Limits per `telemetry/receiver`: body size (4 MiB), requests per minute per key (600) and concurrent requests (8).
  Redaction runs before indexing, with one rule for sensitive names (`context/identity.rs`) for parameters, URL
  queries and attributes.
- Cumulative metrics (Prometheus style) become deltas through a bounded cache of the last point per series; a lower
  value or a new start time is a reset. Exponential histograms become deltas the same way, and are merged (scaled down
  to common buckets) for the hour and day rollups.
- External data lands in the same indexes, told apart by `service.name` and `service.instance.id`, so every screen
  works on it unchanged.

**Out.** `otlp-endpoint` (for example `http://collector:4318`) also sends everything to an OpenTelemetry Collector or
any OTLP/HTTP backend. `otlp-protocol` is `http/protobuf` (the default) or `http/json`. Each request groups its
records by resource and scope. `otlp-headers` is a comma-separated `name=value` list; header values are never
logged. The remote export runs on its own thread with a bounded queue (8,192 records, 64 batches; 2 s connect and 5 s
request timeouts), so a slow or down collector drops and counts records (`remote_dropped`) and never holds up local
storage. Local storage and the remote export fail separately and are counted separately.

**Scrape.** `/metrics` answers Prometheus text (format 0.0.4) for `?format=prometheus` or a Prometheus `Accept`
header, and the plain `name value` lines otherwise. The text comes from a cumulative reader of this server's own
instruments (metrics received over OTLP are not on it), plus the pool, statement, guard and telemetry loss totals,
and a `target_info` series with the resource. It has the same address gate as `/health`, and a public site never
takes either path.

## Deploy markers and annotations

Annotations are change markers that time series panels draw as lines and bands: deploys, config changes, releases,
alert state changes and notes.

- CI posts JSON to `POST /v1/annotations` with an ingest key: `time` (default now), optional `timeEnd`, `source`
  (`deploy`, `config`, `release` or `user`), `text`, `tags`, and optionally `serviceName`, `instanceId`, `version`,
  `revision`, `dashboardId` and `panelId`.
- A signed-in user or a script with a login key can post the same to `/rest/s1/moqui/monitor/annotations`.
- Dashboards › Annotations lists them and adds notes by hand.

## The Monitor app

Monitor (menu index 97, `MONITOR_APP`) is mostly dashboards shipped from `tools/report/` and Explore presets.

| Screen | What it shows |
| --- | --- |
| Overview | Request rate and duration, job runs, top error groups and telemetry loss |
| Services | Services, endpoints and instances (with version) as tables that narrow the dashboard when clicked; request rate, 5xx errors and duration percentiles over time; the busiest artifacts, linking to Artifacts |
| Traces | Requests (SERVER root spans) by default, or all spans, searched by service, span name (`*` wildcard), status, duration range, user and a query; TraceDetail draws the waterfall, span attributes and the trace's logs |
| Logs | Explore over `otel_logs`: query, filters, paging both ways, context lines, a live tail, the trace link per line, CSV |
| Errors | Failed requests and jobs over time, error groups by fingerprint, then samples, the stack and their traces and logs |
| Compare | Two ranges or versions side by side: requests, 5xx errors and duration percentiles per `service.version`, and the documents |
| Metrics | A metric picker (statistic, including p50/p95/p99 from the buckets, and group by an attribute) over Explore on `otel_metrics` |
| Artifacts | Hits, total, min, max, average, p50/p95/p99, std dev, slow hits and last active minute per artifact, by type, sub type and name, plus the same stats over time |
| Sources | Ingest keys (a new key's token shows once, in a message; the log masks it), the latest data from each source, exporter status and loss counters |

The Monitor screens built on Dashboards screens run the Explore and Dashboard transitions themselves, so trace, user,
export, inspect and tail links work for a user with `MONITOR_APP`; Log context and Compare are mounted under Monitor
for the same reason. The System app's Visit screens list a visit's
requests from the request spans, and a job run links to its trace and its logs.

## The Dashboards app

Dashboards (menu index 96, `DASHBOARDS_APP`) holds the general tool.

- **Datasets** are what a query may read: a DataDocument (with its time field), an index-wide `index:NAME` dataset
  over every document type of an index, or a telemetry index. A dataset definition can add field defaults (unit,
  decimals, entity lookups, status and enumeration labels) and row filters.
- **Explore** queries one dataset: a query string (the Lucene subset), filter pills (pin, negate, disable, label),
  a field sidebar with top values, a count over time, the document table, Split to compare two ranges or filters,
  saved searches, and "add to dashboard".
- **Dashboards** are panels and a layout. Panels run report queries (aggregations by role: metric, x-axis, split
  series, split rows), then transformations, then a visualization: time series, bar, stat, gauge, bar gauge, table,
  pie and donut, heatmap, state timeline, text and the trace waterfall. Variables sync to the URL as `var-NAME`, along
  with the time range (`now-6h`, `now/d`, `now-1M/M`, `now/fy`; `<report week-start fiscal-start-month
  fiscal-start-day/>` in the conf sets the week and fiscal year). Keys: `t-` zooms out, `t←`/`t→` shift the range,
  `e` edits, `v` views a panel, `i` inspects, `d k` toggles kiosk mode, and Esc leaves kiosk mode, then the single-panel
  view (in the TUI it closes the inspector first). Playlists rotate dashboards in kiosk mode for a wall display.
  The time picker has quick ranges, the viewer's recent ranges and an absolute range. Each panel's menu has view,
  edit, share (a link with the time made absolute), explore, inspect, duplicate, remove and new alert rule. The
  editor suggests visualizations from the query's or the data's columns. Tables can have footer calculations
  (computed from every row, not the ones shown) and sparkline cells; text panels take `${name}` variables and
  `${__from}`/`${__thru}`, escaped as text; a heatmap draws latency from histogram buckets. The TUI draws the same
  charts with block characters, formats currency, and has kiosk mode and auto refresh.
- **Embedding.** `<report-embed definition-id="" panel-id="" from="now-7d" thru="now" variables="[map expression]"/>`
  places a saved dashboard, one of its panels or a library panel on any screen. It renders as the viewer, so a
  visitor who is not signed in (such as on a public site) gets nothing, and one that cannot be read logs a warning
  and renders nothing.
- **Definitions.** Dashboards, library panels, datasets, saved searches, alert rules and report schedules are JSON in
  `moqui.report.Definition`, with versions (diff and restore) and references (the "used in" lists, export and
  import). Components ship them as `report/*.json`, one file each with `id`, `type`, `title` and `body`; shipped ones
  are read-only until saved as a copy. A shipped alert rule is a template: it runs only once copied, which gives it an
  owner.
- **Alerts.** A rule is a report query, a reduce to one value per series, and a condition (above, below, within or
  outside, with an optional recovery threshold). The `evaluate_ReportAlerts_minute` job runs each rule at its own
  interval, as its owner. States: Normal, Pending, Firing, Recovering, No Data and Error; missing or failed data never
  counts as healthy. A series missing for `pruneAfterMissing` evaluations in a row (10 by default) is removed, with a
  resolved notification if it was not Normal; a rule tracks at most 500 series. Contacts are the `ReportAlert`
  notification topic (or another), email through an `EmailTemplate`, and webhooks (which need `REPORT_CONTACT_ADMIN`;
  credentials come from server properties, never from the rule). Group wait and repeat are set per contact point.
  Contact points, silences and mute timings are set in the rule or shared as their own definitions (Dashboards ›
  Alerts), which rules name in `contactPointRefs`, `silenceRefs` and `muteTimingRefs`. A rule's query and deliveries
  run outside its lock, with rules evaluated on `alert-workers` threads (`<report>`, default 4). Relative ranges use
  the evaluator's effective time. State changes are written as annotations. mr-mantle ships a "Daily sales below
  floor" rule template.
- **Reports.** A report schedule extends `ScreenScheduled`: one or more dashboards, each with its own range and
  variables, rendered by the server's Chromium to PDF (`orientation` portrait or landscape, `layout` grid or rows,
  `appendix` for table data, `paper` letter or a4), a PNG inline in the email with a link to the live dashboard, CSV
  or xlsx: one CSV, or one sheet, per table panel (a dashboard with no table panel exports each panel with data), and
  a preview of several CSVs downloads as a zip. Preview, Send now and Pause are on Dashboards › Reports. An empty report is skipped. Saving to a server
  location needs `REPORT_SAVE_LOCATION`.

## Security

- Every query runs as the viewer. A dataset is authorized as the artifact `org.moqui.report.dataset.<id>`
  (`MONITOR_APP` holds the `otel_*` ones; `REPORT_DATASETS_ADMIN` holds all), plus `DataDocumentUserGroup` for a
  DataDocument. A saved panel never widens access.
- Row filters (`rowFilter`, `contextFilters` from the signed-in identity, or a `rowFilterService`) are added by the
  server before documents, ranking, totals, aggregations and exports; a client never supplies them. Existing entity
  filters are not translated automatically: write the dataset filters an organization needs, and grant business roles
  only the datasets they should read.
- Alerts and scheduled reports run as their owner, whose access is checked at each run; recipients do not set what
  the report may read.
- Queries have row, bucket and time limits, and a timeout. Exports and scheduled deliveries are logged to the
  `moqui.report.audit` target, so they show in Monitor Logs.

## Capacity and loss

Telemetry never blocks or fails business work. Records go to a bounded queue (8,192 records and 32 MiB by default,
with 512 and 2 MiB of that kept for request and job roots) and are written by one worker thread, never inside a business transaction.
When the queue is full or the store is down, records are dropped and counted: `/metrics` and Monitor › Sources show
queued, exported, dropped and failed counts for the local store and the remote exporter; `failed` is the records a store
refused or could not take (part of `dropped`), and `/metrics` shows it as `telemetry_export_failures_total`. Missing
telemetry shows as missing, never as healthy. On OpenSearch, telemetry writes don't wait for a refresh, so new spans can
take the index refresh interval to appear.

## Upgrading

- Deploy the Rust build together with the framework and tools component changes. The Mantle sales dashboard and its
  grants are in mr-mantle; the BOSS Insights dashboards route is in mr-boss.
- A new database loads `TelemetryData.xml`, `ReportSetupData.xml` and `ReportEmailData.xml` from the conf's `load-data`
  list (the retention, rollup and alert jobs, the `ReportAlert` topic and the report email template). Before the review
  pass they were not on that list, so a database created then lacks them: load them as below.
- The entity check creates the `moqui.report` and `moqui.monitor` tables, and adds `claimToken` and `claimedUntil` to
  `AlertRuleLock`. `ScreenScheduled` gains report definition,
  owner and pause fields, and `ScreenScheduledLock` gains lease fields. On an existing database, review any records
  you customized with the same ids, then load only the new seed files:

  ```sh
  moqui load location=runtime/base-component/framework/data/TelemetryData.xml
  moqui load location=runtime/base-component/framework/data/ReportSetupData.xml
  moqui load location=runtime/base-component/framework/data/ReportEmailData.xml
  moqui load location=runtime/base-component/tools/data/MonitoringSecurityData.xml
  # Only when mr-mantle is installed:
  moqui load location=runtime/component/mr-mantle/data/MonitoringSalesSecurityData.xml
  ```

  Then set the report email template's SMTP server.
- **Retired:** the ArtifactHit and ArtifactHitBin entities and their report view, the System Artifact Summary,
  Artifact Bins, Log Viewer and DataDocument Search screens, the hit REST resources, and the shipping of logs to
  `moqui_logs` and `moqui_http_log`. Startup pauses the old `clean_ArtifactData_daily`,
  `clean_ElasticSearchLogMessages_daily` and `clean_ElasticSearchHttpLogMessages_daily` jobs (keeping their past
  runs). Visit and Visitor records stay.
- **Old data** is not migrated or dropped. After a backup, and only once you no longer need the old rows, Postgres
  installations may drop the tables with `DROP TABLE IF EXISTS artifact_hit;` and
  `DROP TABLE IF EXISTS artifact_hit_bin;` (adjust schema and names). Startup never runs these.
- **Telemetry mappings** are checked at start; a changed mapping is refused rather than rebuilt over existing data.
  Back up and migrate, or point at a fresh prefixed store, before upgrading across a mapping change.
- **Alert notifications.** The `ReportAlert` topic ships with `receiveNotifications="Y"`; a database that loaded it
  before the review pass has `N`, which delivers to nobody who has not opted in. Reload `ReportSetupData.xml` or edit
  the topic.
- **Site confs.** `server-stats/@stats-skip-condition` and `artifact-stats` are deprecated and only warn at start. The
  confs in `runtime/conf` no longer set them; a site conf copied from an older one should drop them too, or move
  `artifact-stats` to `telemetry/artifact` as `MoquiDevConf.xml` now does.
- **Defaults changed** in the review pass: the sample ratio is now `0` (set `telemetry_sample_ratio=1` to keep every
  span), and the browser no longer carries the page's `traceparent`, so a session's requests are separate traces
  (find them together by `moqui.visit.id` or `moqui.page.trace_id`).

## Not built

The open items are in [Remaining-Work](../plans/Remaining-Work.md#monitoring), among them overhead at production scale.
Browser RUM, profiling, incidents and recovery are the Later stages of the plan.
