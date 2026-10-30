# Monitoring, dashboards and reports

Moqui records its own work as OpenTelemetry spans, metrics and logs, takes in OTLP from other systems, and keeps all of
it in SearchFacade indexes on Postgres or OpenSearch. The Monitor app reads those indexes. The Dashboards app is a
general reporting tool over the same indexes and over business DataDocuments: datasets, report queries, panels,
dashboards, alerts and scheduled reports. The design and the build record are in
[plans/Monitoring.md](../../plans/Monitoring.md). As of Oct 27, 2026 (M1–M7 and the review pass built).

The code: `crates/moqui-core/src/telemetry/` (the SDK wiring, sampling, the bounded worker, OTLP in and out, rollups
and retention), `crates/moqui-report/src/` (datasets, queries, frames, transformations, definitions, alerts,
annotations, the scheduled reports in `schedule.rs` and the ingest keys in `monitor.rs`),
`crates/moqui-core/src/search/telemetry.rs` (the index mappings), and the host side in `crates/moqui-host/src/http.rs`
(request spans) and `crates/moqui-host/src/monitor/` (the OTLP receiver: `receiver.rs` over HTTP, `grpc.rs` over gRPC). Screens:
`runtime/base-component/tools/screen/Monitor/` and `Dashboards/`; widgets in
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
- **Business operations.** A service call that owns its transaction (the outermost, or a `force-new` one) is an
  operation, named by the service, by its `operation` attribute, or by a `telemetry/operations` rule. Each run counts
  as `moqui.operation.runs`, and its time as `moqui.operation.duration`, by `moqui.operation.name` and `outcome`, taken
  from the transaction's commit and rollback hooks: `commit`, `rollback`, `retry` (rolled back, and a scheduled
  ServiceJob or a SystemMessage send or consume under its retry limit will run it again) or `pending` (queued for a
  worker, counted once when queued). A call that joins its caller's transaction counts nothing of its own, and a
  `transaction="ignore"` service is never an operation. The service's span carries `moqui.operation.name`,
  `.outcome` and `.ref`, the correlation reference `parameter:value` from `operation-ref` (an id, never document
  contents, and never a metric label). The framework's own `org.moqui.*` services are left out by the default conf.
- **Logs.** Every INFO and higher log line, with the trace and span ids of the frame that wrote it.
- **Errors.** Each error span or exception is fingerprinted (type, normalized message, top frames) into `otel_errors`,
  counted once per failing request.
- **Browser (RUM).** With `<telemetry><browser enabled="true">`, a signed-in qvt page (`js/WebrootRum.js`, loaded by
  `qapps.xml` only then) sends a `documentLoad` span with its web vitals (`browser.web_vital.ttfb`, `.fcp`, `.lcp`,
  `.cls`, `.inp`) and a child span per resource slower than a second, a `navigation` span per in-app page change, a
  CLIENT span per request to the site (which sends a `traceparent`, so the server's request span is its child and one
  navigation is one trace), and its JavaScript errors (window, promise and Vue) as ERROR logs. Each tab session is in
  or out by `sample-ratio` (errors by `error-sample-ratio`). The page posts OTLP/JSON to `/rum/traces` and `/rum/logs`
  every 10 seconds and when it is hidden, with the session's CSRF token; anonymous pages send nothing. The server
  replaces the page's resource with its own (`service.name` is `service-name`, by default the server's with
  `-browser`; `user.id` and `moqui.visit.id` from the session; `source.id` `browser`), drops record attributes that
  name a user, visit, source or service, and keeps only CLIENT and INTERNAL spans, at most 512 records a post, within
  `body-limit` (256 KiB) and `requests-per-minute` (120) per user and visit. Only URL paths are sent, never queries.
- **Session replay.** With `replay-enabled="true"` as well, a `replay-sample-ratio` (0.1) of tab sessions records an
  rrweb replay (`libs/rrweb`, loaded only then): every input and all text masked in the browser, except text under
  `replay-unmask-selector`; `[data-moqui-block]`, canvas, video, iframes and `replay-block-selector` recorded as empty
  boxes; no console or network capture. Chunks go to `/rum/replay` until `replay-max-minutes` (30) or
  `replay-max-bytes` (5 MiB), which the server holds each replay to; it keeps them gzipped in `browser_replays` for
  `replay-retention-days` (7). Replays are never a report dataset: Monitor › Replays reads them through services that
  need `MONITOR_REPLAY` (ADMIN has it), and each one opened is logged.

The indexes are `otel_spans`, `otel_logs`, `otel_metrics`, `otel_errors`, `monitor_annotations` and
`browser_replays`.

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
  roots and the tail rule are unchanged. A SQL span keeps its frame's decision, so a frame sampled by its type's ratio
  keeps its statements; other child spans follow the global ratio.
- **`metric`** sets the attributes kept on one instrument, replacing the global allow-list for it; `attributes=""`
  keeps none. Instruments without one keep the global list.
- **`operations`** chooses the business operations: `record="default"` (every call that owns its transaction),
  `"declared"` (only services with `operation`, or that a rule matches) or `"none"`, and `max-names` (500; later names
  are recorded as `other`). Each `<operation service="regex" name="..." ref="..." record="false"/>` rule applies to
  the services whose full names match; the first match wins, and a component's rule replaces the framework's of the
  same pattern. A service declares its own with `operation="order.place" operation-ref="orderId"`.
- **New fields.** A telemetry index whose mapping only gained fields gets their columns (Postgres) or properties
  (OpenSearch) added in place when the server starts; rows written before have them empty. Any other change to a
  telemetry mapping is refused, so history is never dropped.
- **Capacity:** `queue-records` (8192), `queue-bytes` (33554432), `tail-traces` (1024) and `tail-trace-spans` (512)
  on `<telemetry>`. A sixteenth of the queue is kept for request and job roots. A bad value warns and keeps the default.
- **Retention** is per index and per metric granularity, in days; the `clean_TelemetryData_daily` job applies it and
  `rollup_TelemetryMetrics_hourly` rolls minutes into hours and days.
- **Day partitions** (Postgres search backend): `<search-facade><postgres partition-telemetry="true"
  partition-days-ahead="3"/>` makes new `otel_spans`, `otel_logs`, `otel_metrics` and `monitor_annotations` tables
  partitioned by UTC day (`<table>_p<yyyymmdd>`, with a `<table>_default` for rows of a day with no partition);
  `otel_metrics` is split by granularity first (`otel_metrics_minute`, `_hour`, `_day`), since each keeps for its own
  time. `otel_errors` is not split. Partitions are made from yesterday to the days ahead when a table is first used
  and by the retention job, which drops the days wholly past retention and then deletes the rows left. A table made
  before the option was on stays as it is, with a warning, until Monitor › Sources › Move telemetry tables into day
  partitions (`org.moqui.impl.TelemetryServices.partition#TelemetryTables`) copies its rows into a partitioned table
  and keeps the old one as `<table>_unpartitioned` to drop.
- **`interval-seconds`** is for tests; the metric documents are minute documents.
- A screen with `track-artifact-hit="false"` records no artifact spans or metrics for itself and anything under it.
  Its request span is still written.
- `server-stats/artifact-stats` from older confs still applies, with a deprecation warning: `persist-hit` turns the
  type's spans on or off and `persist-bin` its metric. An explicit `telemetry/artifact` wins.

## OTLP in and out

**In.** `POST /v1/traces`, `/v1/logs` and `/v1/metrics` take OTLP/HTTP as `application/x-protobuf` or
`application/json`, gzip allowed, and answer as OTLP does, with partial success. In a build with the `grpc` feature
(Enterprise and Maximal), `<telemetry><receiver grpc-enabled="true">` also opens OTLP/gRPC on `grpc-port` (4317) at
`grpc-bind` (the HTTP listener's address by default); the dev conf turns it on. It is TLS with the HTTPS listener's
certificates, else plain h2c, takes gzip, and answers refusals as gRPC statuses (`UNAUTHENTICATED`,
`RESOURCE_EXHAUSTED`, `UNAVAILABLE`, `INVALID_ARGUMENT`; `OUT_OF_RANGE` over the body limit).

- Each sender needs an ingest key: Monitor › Sources › Create key, which shows the token once. Send it as
  `Authorization: Bearer <token>` or an `api_key` header (gRPC metadata of the same names). A key names its source, which is recorded on everything it
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
any OTLP/HTTP backend. `otlp-protocol` is `http/protobuf` (the default) or `http/json`, and `otlp-compression` is
`gzip` (the default, sent as `Content-Encoding: gzip`) or `none`. Each request groups its records by resource and
scope, and batches of one signal waiting in the queue are merged into one request of up to 2,048 records.
`otlp-headers` is a comma-separated `name=value` list; header values are never logged. The remote export runs on its
own thread with a bounded queue (8,192 records, 64 batches; 2 s connect and 5 s request timeouts), so a slow or down
collector drops and counts records (`remote_dropped`) and never holds up local storage. Local storage and the remote
export fail separately and are counted separately.

**Scrape.** `/metrics` answers Prometheus text (format 0.0.4) for `?format=prometheus` or a Prometheus `Accept`
header, and the plain `name value` lines otherwise. The text comes from a cumulative reader of this server's own
instruments, plus the pool, statement, guard and telemetry loss totals, and a `target_info` series with the resource.
After them come the metrics received over OTLP and from scrape targets: sums and histograms added up since start
(a monotonic sum is a `_total` counter), gauges at their last value, each labelled by its attributes and
`service_name`, `service_instance_id` and `source`. At most 5,000 received series are kept; a series past that, or a
family named like one of this server's own (so scraping this server's own `/metrics` adds nothing), is left out and
counted in `moqui_telemetry_received_series_dropped_total`. Exponential histograms are not on it. It has the same
address gate as `/health`, and a public site never takes either path.

**Pull.** Monitor › Checks holds two kinds of pulled telemetry, run by ServiceJobs each minute; each target or check
runs when its own `intervalSeconds` (60 by default) has passed, and a failure is recorded on its row and logged without
holding up the others.
- **Scrape targets** (`moqui.monitor.ScrapeTarget`, `scrape#Targets`, job `scrape_TelemetryTargets_minute`): a
  Prometheus-text or OpenMetrics endpoint, read with an optional bearer token (stored encrypted) into `otel_metrics`
  with `source.id` `scrape:<targetId>`, `service.name` from the target (its `host:port` when empty) and
  `service.instance.id` `host:port`. Samples become OTLP points and pass through the receiver's decoder: counters and
  histograms are cumulative, so a series' first scrape is its baseline (not counted as loss) and later scrapes are
  deltas; a counter that goes down is a reset. A summary becomes a gauge per quantile plus `_sum` and `_count` counters.
  Each scrape also writes `up` (1 or 0), `scrape_duration_seconds` and `scrape_samples_scraped`. The body is capped at
  8 MiB and 10,000 points; NaN values and histograms whose buckets do not add up are skipped.
- **Synthetic checks** (`moqui.monitor.SyntheticCheck`, `run#Checks`, job `run_SyntheticChecks_minute`): an HTTP GET,
  HEAD or POST with an expected status (200 by default) and optional body text. Each run is the root of its own trace,
  `check <name>` with `moqui.synthetic.check`, `synthetic.check.id` and `synthetic.check.name`, kept and head-sampled
  whatever the sample ratio, with the request as its CLIENT child carrying `traceparent`, so a Moqui or other traced
  target joins the trace. A wrong status, missing body text or a transport error marks it ERROR. (When a check calls a
  page of this same server, that page's request span, unless sampled, decides the trace first and the CLIENT child is
  left out; the check span and the page's span are still kept.)
- The shipped `monitor-checks` dashboard shows runs and mean duration per check, failures, runs by status over time,
  and scrape targets' `up`. Targets and checks are set by a `MONITOR_APP` user and may be internal addresses.

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
| Operations | Business operations: runs ended, commits, rollbacks, retries and pending per operation, with average and p50/p95/p99 duration; outcomes over time, rollbacks and retries by operation, and duration percentiles. An operation narrows the dashboard to it |
| Services | Services, endpoints and instances (with version) as tables that narrow the dashboard when clicked; request rate, 5xx errors and duration percentiles over time; the busiest artifacts, linking to Artifacts |
| Traces | Requests (SERVER root spans) by default, or all spans, searched by service, span name (`*` wildcard), status, duration range, user and a query; TraceDetail draws the waterfall, span attributes, a flame graph of self time by span path, and the trace's logs |
| Logs | Explore over `otel_logs`: query, filters, paging both ways, context lines, a live tail, the trace link per line, CSV, and log patterns |
| Errors | Failed requests and jobs over time, error groups by fingerprint, then samples, the stack and their traces and logs |
| Compare | Two ranges or versions side by side: requests, 5xx errors and duration percentiles per `service.version`, and the documents |
| AI | Traces with LLM calls: calls, input and output tokens, models, providers, the slowest call and errors per trace, named by the agent, gateway or assist frame that made them; filters by provider, model, frame name and errors; links to TraceDetail (whose waterfall shows each call's model and tokens), the AI dashboard and System › LLM › Usage for cost |
| Metrics | A metric picker (statistic, including p50/p95/p99 from the buckets, and group by an attribute) over Explore on `otel_metrics` |
| Artifacts | Hits, total, min, max, average, p50/p95/p99, std dev, slow hits and last active minute per artifact, by type, sub type and name, plus the same stats over time |
| Sources | Ingest keys (a new key's token shows once, in a message; the log masks it), the latest data from each source, exporter status and loss counters |
| Checks | Synthetic checks and Prometheus scrape targets: add, edit, enable, run now, last result, and links to each check's runs and last trace, a target's metrics and the checks dashboard |
| Browser | Page loads and p75 largest contentful paint over time; slowest pages with p75 load, LCP, INP and CLS; in-app navigations; the browser's requests (p95 by path); JavaScript errors by page and type |
| Releases | With `MONITOR_INCIDENT`: each service's versions from its release markers, newest first, with the instances reporting each now, its open regressions and a link to Compare with the version before; ReleaseDetail lists the regressions (baseline and candidate values, change, confidence, severity, data quality, where) and the error groups first seen since the release |
| Incidents | With `MONITOR_INCIDENT`: open incidents (or all, or of a status or service), New incident; IncidentDetail shows severity, status, onset and the cause's status, the ranked causes with what supports and contradicts each (Confirm, Reject), one timeline (releases and changes, regressions, new errors, failing traces, missing telemetry, alerts, decisions, actions, verifications and notes, with links to traces and error groups), the recovery panel (the next safe action, its blast radius, why automation stopped, Approve and Refuse; actions with Mark done, Mark failed, Cancel and Revert) and the regressions |
| Recovery | The emergency stop, the recovery policies and adapters (changed with `MONITOR_RECOVER_ADMIN`), and the newest actions |
| Replays | With `MONITOR_REPLAY`: session replays by user and visit, newest first; ReplayDetail plays one (rrweb's Replayer, with play, pause and seek; the terminal client lists its page and traces) and links the traces its page touched. The Visit screen links a visit's replays |

The Monitor screens built on Dashboards screens run the Explore and Dashboard transitions themselves, so trace, user,
export, inspect and tail links work for a user with `MONITOR_APP`; Log context and Compare are mounted under Monitor
for the same reason. The System app's Visit screens list a visit's
requests from the request spans, and a job run links to its trace and its logs.

## The Dashboards app

Dashboards (menu index 96, `DASHBOARDS_APP`) holds the general tool.

- **Datasets** are what a query may read: a DataDocument (with its time field), an index-wide `index:NAME` dataset
  over every document type of an index, or a telemetry index. A dataset definition can add field defaults (unit,
  decimals, entity lookups, status and enumeration labels) and row filters. A lookup reads its label table (a
  product's name, a status or unit description) without an entity grant of its own: it only labels ids already in the
  authorized frame. `unitField` names the column an amount's unit comes from (mr-mantle's amounts use
  `currencyUomId`, with `USD` as the fallback `unit`): one currency in the frame is the field's unit, several give each
  row its own, and an aggregation without the column asks the same documents for their currencies, so a sum in one
  currency shows it and a sum across currencies shows a plain number.
- **Explore** queries one dataset: a query string (the Lucene subset), filter pills (pin, negate, disable, label),
  a field sidebar with top values, a count over time, the document table, Split to compare two ranges or filters,
  saved searches, and "add to dashboard".
- **Log patterns.** On a dataset with a `body` field (Monitor › Logs), "Show log patterns" groups the newest 1,000
  matching lines by template, after Drain: numbers, ids, hex and quoted text become `<*>`, lines of the same length and
  first word are compared word by word, and a line joins the closest template when at least half its words agree. Each
  pattern shows its count, share, most common level, latest time and newest line, and clicking it narrows the search to
  the pattern's constant words. Search highlights show in the row as marked text. The same grouping is the `patterns` transformation (`field`, `limit`, `buckets`) on a
  documents-mode panel query, which adds `first`, `last`, a `sparkline` of counts over time and the narrowing `query`.
  It reads at most 5,000 lines and keeps 200 patterns; the rest join an `(other)` pattern.
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
  `${__from}`/`${__thru}`, escaped as text, and a data link takes `${__from}`/`${__thru}` too, so a drill-down (such as
  the Services dashboard's) keeps the range; a heatmap draws latency from histogram buckets (the Services dashboard has
  one). The TUI draws the same charts with block characters, formats currency, and has kiosk mode (↑/↓ go a panel at a
  time, PgUp/PgDn scroll) and auto refresh; `p` opens its time picker of quick ranges, `m` the selected panel's menu
  (Remove asks for a second Enter), and its editor lists the suggested visualizations first, with why. Titles of
  dashboards, panels, rows, tabs and variables, and the widgets' own text, show in the viewer's language when the
  catalog has them (mr-l10n).
- **Repeats.** A panel, or a row or tab item, with `repeat: {variable, direction: "h"|"v", maxPerRow}` is copied once
  for each selected value of the variable (for All, each of its options), with the variable pinned to that value in
  the copy's queries, title and text. Repeats are expanded on the server, so the browser, the TUI and the print
  document get the same panels. The first copy keeps the panel's id, and the others are `<id>~<n>` (`<id>~<row>-<n>`
  inside a repeated row). The single-panel view and exports work on any copy, and edit, duplicate and remove act on
  the panel being repeated. A horizontal repeat in a grid places `maxPerRow` copies (4 by default) on each row and
  moves the items below it down. One repeat makes at most 24 copies and a dashboard has at most 100 panels; past
  either limit a notice says what was left out.
- **Funnel, node graph, flame graph and geomap.**
  - A funnel takes a category and a number, or a single row of numbers. Each step shows its share of the first step
    and of the one before.
  - A node graph takes `source`, `target` and `count` rows (and `errors` and `duration` when present). It draws
    layered nodes, with the edges that close a cycle drawn back.
  - A flame graph takes `path` rows (`a;b;c`) and a value. Click a frame to zoom into it.
  - A geomap draws points from latitude and longitude fields, sized by a value. Without those fields, it shades
    countries from a country field: a Moqui `geoId` (ISO alpha-3), alpha-2 or a numeric code. The outline is Natural
    Earth's 110m countries (`libs/naturalearth`, inlined into the print document). The TUI plots braille points over
    a land mask.
  - Two query modes over spans feed the first two:
    - `mode: "serviceMap"` gives the calls between services: a span whose parent is in another service, a CLIENT span
      to a database (`db:<system>`), and a CLIENT span to a `server.address` that no span answered.
    - `mode: "flame"` gives the self time of each span-name path.

    Both read at most the newest 10000 spans of the range, as the viewer, with the query string, filters and
    variables applied. Monitor ships a Service map dashboard, and Trace detail shows the trace's flame graph.
- **Git sync** (Dashboards › Git sync, `REPORT_GIT_SYNC_ADMIN`) keeps a definition folder (`body.folder`) in a
  directory of a git repository, `git://<repositoryId>/<path>/<id>.json`, one file per definition in the shipped
  format.
  - Saving a definition in the folder writes its file once the save commits.
  - A push or fetch to the repository's branch saves the files that changed, by their ids (the
    `FrameworkPullDefinitionGitSyncs` gitECA). The sync's own writes are not pulled back, and a file that already
    matches is skipped, so nothing loops.
  - Push now and Pull now are on the screen, along with the last push, the last pull and the last error.
  - Both run as the sync's user, so its git and definition rights apply: a definition another user owns is not saved.
  - A file that fails is named, and the rest still sync.
  - Deletes: each definition a sync wrote or read is tracked (`DefinitionGitFile`). A pull deletes a tracked definition
    whose file was removed (as the sync's user, so only one it owns and nothing still references); an untracked one is
    only listed, and a directory with no definition files deletes nothing. Deleting a tracked definition removes its
    file once the delete commits, and Push now removes any left behind.
- **Embedding.** `<report-embed definition-id="" panel-id="" from="now-7d" thru="now" variables="[map expression]"/>`
  places a saved dashboard, one of its panels or a library panel on any screen. It renders as the viewer, so a
  visitor who is not signed in (such as on a public site) gets nothing, and one that cannot be read logs a warning
  and renders nothing.
- **Reusing the dashboard list.** `Dashboards/Home.xml`'s links run through its own transitions (`dashboard`, `star`,
  `edit`, `new`, `explore`), so a screen that includes it takes them in with `transition-include`, as
  `Dashboards/Panels.xml` and BOSS Insights › Dashboards do. A dashboard then opens in that screen's sibling `Dashboard`
  screen (in BOSS, Insights › Dashboard); editing and Explore open the Dashboards app.
- **Definitions.** Dashboards, library panels, datasets, saved searches, alert rules and report schedules are JSON in
  `moqui.report.Definition`, with versions (diff and restore) and references (the "used in" lists, export and
  import). Components ship them as `report/*.json`, one file each with `id`, `type`, `title` and `body`; shipped ones
  are read-only until saved as a copy. A shipped alert rule is a template: it runs only once copied, which gives it an
  owner. The owner can delete a saved definition (`delete#Definition`; Delete on the Definition, Alerts and shared item
  screens) with its versions and an alert rule's state; a shipped one cannot be deleted, nor one that other definitions
  still reference (the error names them). Deletes are logged to `moqui.report.audit`.
- **Alerts.** A rule is a report query, a reduce to one value per series, and a condition (above, below, within or
  outside, with an optional recovery threshold). The `evaluate_ReportAlerts_minute` job runs each rule at its own
  interval, as its owner. States: Normal, Pending, Firing, Recovering, No Data and Error; missing or failed data never
  counts as healthy. A series missing for `pruneAfterMissing` evaluations in a row (10 by default) is removed, with a
  resolved notification if it was not Normal; a rule tracks at most 500 series. Contacts are the `ReportAlert`
  notification topic (or another), email through an `EmailTemplate`, and webhooks (which need `REPORT_CONTACT_ADMIN`;
  credentials come from server properties, never from the rule). Group wait and repeat are set per contact point.
  Contact points, silences and mute timings are set in the rule or shared as their own definitions (Dashboards ›
  Alerts), which rules name in `contactPointRefs`, `silenceRefs` and `muteTimingRefs`. A shared silence with matchers
  also mutes every rule whose owner may read it (the owner's own, or one shared with their group) and whose series
  labels match, referenced or not; one with no matchers mutes only rules that reference it. A delivery is recorded as
  sent just before it goes out, so a retry after a failed state write does not send the same state again (a failed
  send is put back for retry). A rule's query and deliveries
  run outside its lock, with rules evaluated on `alert-workers` threads (`<report>`, default 4). Relative ranges use
  the evaluator's effective time. State changes are written as annotations. mr-mantle ships a "Daily sales below
  floor" rule template.
- **Reports.** A report schedule extends `ScreenScheduled`: one or more dashboards, each with its own range and
  variables, rendered by the server's Chromium to PDF (`orientation` portrait or landscape, `layout` grid or rows,
  `appendix` for table data, `paper` letter or a4), a PNG inline in the email with a link to the live dashboard, CSV
  or xlsx: one CSV, or one sheet, per table panel (a dashboard with no table panel exports each panel with data), and
  a preview of several CSVs downloads as a zip. Preview, Send now and Pause are on Dashboards › Reports. An empty report is skipped. Saving to a server
  location needs `REPORT_SAVE_LOCATION`.

## Regressions and incidents

The `detect_Regressions_5min` job (`detect#Regressions`) compares each service version that served requests in the
last `window-minutes` with a baseline:
- **The version before**, over `lookback-hours` before the release marker, while the release is under `watch-hours`
  old. These stats are kept (`moqui.monitor.MonitorBaseline`), so the comparison does not drift as raw telemetry
  ages out.
- **The same version** `historical-offset-hours` earlier, after the watch period, or for the first version.

It compares three metrics:
- the error rate (5xx over requests), by a one-sided two-proportion test with Wilson intervals
- p95 request latency, from the merged duration buckets; the p95 must reach a higher bucket, at `latency-ratio` or
  more, with a chi-square test over the buckets
- the operations' rollback rate (rollbacks and retries over runs)

Verdicts:
- A rise counts when its p-value is under `alpha` and it is at least `rate-delta` (one point by default). It is
  critical from `critical-rate` or `critical-ratio`.
- Fewer than `min-samples` on a side gives no verdict.
- A window with fewer than `min-coverage` of its minutes reported, or telemetry the candidate dropped, marks the
  verdict degraded. Neither is ever read as healthy.
- Each instance of the version is compared too: a rise on every instance is version-wide, on some while others are
  healthy an instance fault, and on one instance alone unknown.

A regression stays open while it lasts and is resolved only by good data. One at `incident-severity` (major) opens an
incident, or joins the service's open one. Each service has one open incident at a time.

The incident's timeline is filled with:
- the regression
- the release, deploy and config markers within the lookback before the onset
- error groups first seen near the onset
- sample failing requests of the version
- a missing-telemetry row when the data was degraded

Its causes are ranked by how close each came before the onset, how its scope fits the affected and healthy instances
(instances still on the old version that also regressed contradict the release), and how much of the deviation it
explains. A cause is a hypothesis until an operator confirms it. When every regression of the incident has resolved,
the incident is Monitoring, and after `resolve-minutes` (30) Resolved; a regression back in that time reopens it.

An alert rule with the label `incident="true"` opens an incident, or joins the open one, when a series starts firing.
Each incident view and change is logged on the `moqui.monitor.audit` target.

```xml
<telemetry><regressions window-minutes="60" watch-hours="24" lookback-hours="24" min-samples="200"
        alpha="0.01" rate-delta="0.01" latency-ratio="1.5" incident-severity="major" resolve-minutes="30"/></telemetry>
```

## Recovery

The `run_Recovery_minute` job (`run#Recovery`) applies the first enabled `RecoveryPolicy`, by order, whose service
pattern and least severity fit each open incident.

A policy's ladder of steps is tried least disruptive first:
- `observe`
- `escalate`
- `pause-alert-rule`
- `pause-job`
- `disable-artifact` (a deny for `ALL_USERS` on an artifact group)
- `isolate-instance`, `pause-rollout` and `promote`
- `restart`
- `rollback` to the version before

A step is refused, with its reason recorded, when:
- the regressions' confidence is under `minConfidence` (0.99)
- the evidence is degraded, for the disruptive steps
- the step does not fit the locality: isolating needs an instance fault on fewer than half the instances, and a
  rollback needs a version-wide regression
- a rollback's versions have different entity schemas (each release marker records `moqui.schema.hash`) or unknown
  ones
- another action is active in the service's scope
- another action finished inside `cooldownMinutes`, or the step would reverse one
- the scope has had `maxActionsPerDay` actions today
- the step was already tried for this incident
- the emergency stop is on

The first step allowed is the decision. If none is allowed, the decision is to escalate (or to observe, after a
recent escalation), with the refusals as why automation stopped.

What happens to a decision depends on the policy's mode:
- **Advisory** (the default) records it.
- **Approval** waits for Approve on the incident, which needs `MONITOR_RECOVER`.
- **Automatic** runs it only with `<telemetry><recovery allow-automatic="true">`, and a rollback only when the release
  is the first-ranked cause with a score of 0.5 or more.

Escalation, which changes nothing, always runs. An approval is bound to the decision: if the incident changed, it is
superseded by a new decision.

An action is saved before it runs, with an idempotency key, while it holds the scope's lease. Each adapter type runs it
differently:
- **in-process** pauses a ServiceJob or alert rule, denies an artifact group, or drains an instance (`/health`
  answers 503 with `draining` so a load balancer stops sending it requests, while it still serves those that reach
  it). Each can be reverted.
- **webhook** posts a signed JSON request (action, target, idempotency key, and in the first attempt a callback
  token) to a deployment controller. It is tried again, with the same key, up to five times. The controller answers
  at once, reports on `POST /v1/recovery` with an ingest key and the token, or is polled at `statusUrl`.
- **manual** waits for an operator to mark the action done or failed.

An action past its deadline is timed out and escalated.

`verifyMinutes` after the action took effect, the service since then is compared with each regression's baseline,
the operations' rollback rate is held to `maxRollbackRate`, and a rollback must show its instances on the target
version:
- Verified moves the incident to Monitoring.
- Failed escalates, so the next step can be decided.
- Too little data is Inconclusive until twice the verify time, then escalated, and never Verified.

The shipped `DEFAULT` policy is advisory and only escalates major and critical incidents to the ADMIN group (its
`contactJson` takes a contact point as alert rules do). No action reverses committed business transactions. The
public demo ends `MONITOR_RECOVER` and `MONITOR_RECOVER_ADMIN`.

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
- Regressions and incidents need `MONITOR_INCIDENT`; approving, refusing, completing, cancelling and reverting
  recovery actions `MONITOR_RECOVER`; policies, adapters and the emergency stop `MONITOR_RECOVER_ADMIN`. ADMIN holds
  all three. Their views and changes are logged on the `moqui.monitor.audit` target.

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
- **Telemetry mappings** are checked at start. A mapping that only gained fields gets them added in place, as
  `moqui.operation.*` and `exception.fingerprint` were on `otel_spans` and `moqui.operation.name` on `otel_metrics`
  (rows written before have them empty); a server of an older build then refuses the grown table until it is
  upgraded too. Any other change is refused rather than rebuilt over existing data: back up and migrate, or point at a
  fresh prefixed store, before upgrading across one.
- **Regressions, incidents and recovery** add the `detect_Regressions_5min` and `run_Recovery_minute` jobs to
  `TelemetryData.xml`, the `MONITOR_INCIDENT`, `MONITOR_RECOVER` and `MONITOR_RECOVER_ADMIN` permissions to
  `MonitoringSecurityData.xml`, and the shipped recovery policy and adapters in `RecoverySetupData.xml`
  (`seed-initial`, so the changes an administrator makes are kept). On an existing database, load those three files.
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

The open items are in [Remaining-Work](../../plans/Remaining-Work.md#monitoring), among them overhead at production scale.
Server-JS profiling is the Later stage of the plan not yet built.
