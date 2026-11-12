# Monitoring, dashboards and reports

Moqui records its own work as OpenTelemetry spans, metrics and logs, takes in OTLP from other systems, and keeps all of
it in SearchFacade indexes on Postgres or OpenSearch. Monitor, a section of the System app, reads those indexes. The
Dashboards app is a general reporting tool over the same indexes and over business DataDocuments: datasets, report
queries, panels, dashboards, alerts and scheduled reports. The design and the build record are in
[plans/Monitoring.md](../../plans/Monitoring.md). As of Oct 27, 2026 (M1–M7 and the review pass built).

The code: `crates/moqui-core/src/telemetry/` (the SDK wiring, sampling, the bounded worker, OTLP in and out, rollups
and retention), `crates/moqui-report/src/` (datasets, queries, frames, transformations, definitions, alerts,
annotations, the scheduled reports in `schedule.rs` and the ingest keys in `monitor.rs`),
`crates/moqui-core/src/search/telemetry.rs` (the index mappings), and the host side in `crates/moqui-host/src/http.rs`
(request spans) and `crates/moqui-host/src/monitor/` (the OTLP receiver: `receiver.rs` over HTTP, `grpc.rs` over gRPC). Screens:
`runtime/base-component/tools/screen/System/Monitor/` and `Dashboards/`; widgets in
`webroot/screen/webroot/js/WebrootReport.js` with TUI twins in `crates/moqui-tui/src/components/report.rs`.

## What is recorded

- **Request spans.** Every HTTP request gets a SERVER root span, whatever the sampling: route, status, user, visitor,
  visit, client address, redacted query, sizes and time to first byte (for a screen sent as it renders, the moment its head went: [Screen output](../ui/Screen-Output.md)). The Visit screens and the Monitor Traces screen
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
- **Resource gauges.** Each minute, from the resource readings ([Run-Deploy](../Run-Deploy.md#resource-watch)):
  `system.cpu.load_average.1m` and `.5m`, `moqui.system.cpu.load.utilization` (the 5-minute load over the processors),
  `process.cpu.utilization`, `process.memory.usage`, `moqui.process.memory.utilization` (of the memory limit in
  force), `system.linux.memory.available`, `process.thread.count`, `process.unix.file_descriptor.count`,
  `system.filesystem.utilization` and `moqui.system.filesystem.available` (one series per filesystem, by
  `system.filesystem.mountpoint`), `moqui.service.pool.active` and `.queued`, and `moqui.request.pool.busy` beside
  `moqui.request.queue.depth` and the `moqui.db.pool.*` gauges. A utilization is a share from 0 to 1. The shipped
  `system-home` dashboard and the `system-db-pool` library panel (folder System) chart them.
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
  Replay JSON requests have their own `replay-body-limit` (1 MiB by default); an explicitly configured `body-limit`
  remains the replay cap unless `replay-body-limit` overrides it. Trace/log defaults stay at 256 KiB, and the webapp's
  request-body cap still applies before the replay service. The replay limit includes the UTF-8 JSON envelope;
  `replay-max-bytes` separately limits cumulative event-array bytes. The recorder sends complete events in order,
  with one request in flight and one bounded pending chunk. An event that cannot fit, a full pending buffer or a
  rejected post stops capture with a concise console diagnostic; it never truncates a snapshot or logs its contents.

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
  has an error or a slow root (over `slow-root-ms`, default 1000) or slow frame; the rest are dropped. At `1` every span is kept. The decision
  is made from the trace id, so services with the same ratio agree. The sampled flag of an incoming `traceparent` is
  not trusted.
- **Cost.** With telemetry on, a small service call costs about half as much again as with it off (about 10 → 14 µs
  after the review pass), and at full sampling a burst of requests can fill the queue on a Postgres store; larger
  screens dilute the fixed cost. Roots mode dropped nothing in the same burst. Raise the ratio for a development server, or for a while when you need full
  traces.
- **`artifact`** turns spans and metrics on or off per artifact type. Entity frames are off by default and screen
  content gets metrics only. `AT_TRANSACTION` is each transaction begun (not one joined): a span inside a trace,
  with its outcome, cache mode (`off`, `hold`, `read-through`), waited round trips and the transaction cache's counts
  (`moqui.tx_cache.held`, `memory_reads`, `db_reads`, `early_flushes`, `batch_statements`, `batch_round_trips`;
  [Services](../logic/Services.md#transaction-cache)), and the `moqui.transaction.duration` metric by `outcome` and
  `moqui.transaction.cache`; the dev conf (`runtime/conf/MoquiDevConf.xml`) adds content spans and entity metrics.
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
  partitions (`org.moqui.monitor.TelemetryServices.partition#TelemetryTables`) copies its rows into a partitioned table
  and keeps the old one as `<table>_unpartitioned` to drop.
- **`interval-seconds`** is for tests; the metric documents are minute documents.
- A screen with `track-artifact-hit="false"` records no artifact spans or metrics for itself and anything under it.
  Its request span is still written.
- `server-stats/artifact-stats` from older confs still applies, with a deprecation warning: `persist-hit` turns the
  type's spans on or off and `persist-bin` its metric. An explicit `telemetry/artifact` wins.

## OTLP in and out

**In.** `POST /v1/traces`, `/v1/logs` and `/v1/metrics` take OTLP/HTTP as `application/x-protobuf` or
`application/json`, gzip allowed, and answer as OTLP does, with partial success. In a build with the `grpc` feature
(every package), `<telemetry><receiver grpc-enabled="true">` also opens OTLP/gRPC on `grpc-port` (4317) at
`grpc-bind` (the HTTP listener's address by default); the dev conf turns it on. The port is the host's gRPC listener,
which the PASS authority's gRPC shares when that is on ([Edge-Node](../../plans/Edge-Node.md) §5.5). It is TLS with the
HTTPS listener's certificates, else plain h2c, takes gzip, and answers refusals as gRPC statuses (`UNAUTHENTICATED`,
`RESOURCE_EXHAUSTED`, `UNAVAILABLE`, `INVALID_ARGUMENT`; `OUT_OF_RANGE` over the body limit). The HTTPS listener
takes the same OTLP/gRPC calls over HTTP/2, so a sender can use the HTTPS port instead of 4317.

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
  and scrape targets' `up`. Targets and checks are set by a `SYSTEM_APP` user and may be internal addresses.

## Deploy markers and annotations

Annotations are change markers that time series panels draw as lines and bands: deploys, config changes, releases,
alert state changes and notes.

- CI posts JSON to `POST /v1/annotations` with an ingest key: `time` (default now), optional `timeEnd`, `source`
  (`deploy`, `config`, `release` or `user`), `text`, `tags`, and optionally `serviceName`, `instanceId`, `version`,
  `revision`, `dashboardId` and `panelId`.
- A signed-in user or a script with a login key can post the same to `/rest/s1/moqui/monitor/annotations`.
- Dashboards › Annotations lists them and adds notes by hand.

### Business events

A domain's event (a price list published, a promotion's dates, a fiscal period closed) is an annotation of source `business`,
which a dashboard over business data draws like any other marker.

- **Writing.** `ReportServices.create#Annotation` with `source: "business"`, `text` (a name of at most 255 bytes, never an
  amount), `time` (and `timeEnd` for a band), `organizationPartyId`, `event` (the event's type), and optionally `recordType` and
  `recordId` (the record behind it) or `link` (a path on this server). It needs the artifact
  `org.moqui.report.annotation.business` (create), `ADMIN`'s; `MONITOR_INGEST` and the ingest keys do not write one. The owner
  of an event writes it from its own service after its commit, with authorization off (mr-mantle's
  `mantle.party.BusinessEventServices.record#BusinessEvent`, registered on commit by the publication, promotion and period close
  services, reads the marker from the stored record). With an `eventKey` and the same `time` the event keeps one marker.
- **Reading.** A domain registers a source: a `ReportAnnotationSource` enumeration whose `enumCode` is a dataset over the
  annotations (`indexName: "monitor_annotations"`) with the domain's row filter (mr-mantle's `mantle-business-events`, scoped to
  the viewer's organizations by `get#OrganizationRowFilter`). A dashboard with a panel over a business dataset reads each
  registered source as the viewer and adds its `business` markers to every panel: the dataset's own artifact and row filter
  decide, a source the viewer may not read adds nothing, and only `business` markers are taken from a source whatever its
  dataset says. A marker carries `event`, `organizationPartyId`, `recordType`, `recordId` and `link`; an app resolves the record
  to a screen by the viewer's own permissions. Technical dashboards show none, and the technical marker list leaves them out.
- The regression spec is `framework/test/ReportAnnotations.spec.js`.

## Monitor

Monitor is a section of the System app: `/apps/system/Monitor/<Screen>`, second in System's menu, opening on Overview.
It is mostly dashboards shipped from `tools/report/` and Explore presets. It has no grant of its own: a holder of
`SYSTEM_APP` uses it, and so reads the telemetry (traces with their users, logs, errors); the permissions named below
(`MONITOR_INCIDENT`, `MONITOR_REPLAY`, `MONITOR_SETTINGS` and the others) each gate one thing inside it. "Monitor › X"
is the short name of its screens.

The System home shows the shipped `system-home` dashboard as its Activity strip. To show another there, set
`system_home_dashboard` to that dashboard's definition id (a conf property, so System › Configuration or the
environment): it gets the home's period and, as its `instance` variable, this server. A reader who may not read it sees
the shipped one.

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
| Settings | Logging and telemetry settings with their on-disk values, changed for a while with `MONITOR_SETTINGS` ([below](#changing-settings-for-a-while)) |
| Replays | With `MONITOR_REPLAY`: session replays by user and visit, newest first; ReplayDetail plays one (rrweb's Replayer, with play, pause and seek; the terminal client lists its page and traces) and links the traces its page touched. The Visit screen links a visit's replays |

The Monitor screens built on Dashboards screens run the Explore and Dashboard transitions themselves, so trace, user,
export, inspect and tail links work for a holder of `SYSTEM_APP` without `DASHBOARDS_APP`; Log context and Compare are
mounted under Monitor for the same reason.

### Monitor's screens in other places

Some of what Monitor has also shows where its reader already is. Monitor's screen or dashboard stays the owner; none
of these copies it.

| From Monitor | Also in | How |
| --- | --- | --- |
| Settings | System › Configuration › Logging and telemetry (`/apps/system/Conf/Logging`) | The same screen mounted a second time; changing still needs `MONITOR_SETTINGS` |
| AI | System › LLM and Agents › Traces (`/apps/system/Llm/Traces`) | The same screen mounted a second time |
| The AI dashboard (`monitor-ai`) | LLM and Agents › Overview | Its `calls` and `tokens` panels over seven days, embedded |
| Artifacts (`monitor-artifacts`) | Tools › Service, a service's detail; Tools › Schema, an entity, while the entity artifact type's metrics are on | Its `activity` panel narrowed to that artifact (its hits over seven days), embedded, with a link to Monitor › Artifacts narrowed the same way |
| The database pool gauges | Tools › Database › Overview | The `system-db-pool` library panel over the last hour, embedded |
| Metrics, incidents, errors, checks, sources | The System home | The `system-home` dashboard embedded as the Activity strip, and the counts of Needs attention, each linking to its Monitor screen |
| Traces | System › Security, a user's detail | A link, Requests by this user |

The three embeds in Tools show only to a reader of the `otel_metrics` dataset (granted with System), since a user may
hold Tools without System; the screen checks before it embeds ([Embedding](#the-dashboards-app)). The System app's
Visit screens list a visit's requests from the request spans, and a job run links to its trace and its logs.

## Agent evals

System › LLM and Agents › Evals (`/apps/system/Llm/Evals`) tests an agent against fixed cases and scores what it does.
The entities are in `framework/entity/AgentEvalEntities.xml`, the services in `org.moqui.agent.AgentEvalServices`
(`AgentEval.js`), and the seed data (types, the built-in scorers, the jobs and the permissions) in
`framework/data/AgentEvalData.xml`.

- **Scorers** (`AgentScorer`) give an output a score from 0 to 1 and a reason, with a pass threshold. A *service*
  scorer is a service with `input`, `output`, `groundTruth`, `runId`, `metadata` and `config` in and `score` and
  `reason` out; Matches expected, Contains (the expected output's lines are phrases) and Called tool (its lines are
  tool names, checked against `AgentToolCall`) are built in. A *judge* scorer asks a model to grade by its rubric,
  with the model bound to the `eval-judge` feature (System › LLM) or `config.model`. A *human* scorer is scored by a
  person.
- **Datasets** (`AgentDataset`, `AgentDatasetItem`) are cases: an input, an expected output and metadata. Each change
  raises the dataset's version and keeps the old items, so an experiment's version stays as it ran. Runs › Save as a
  dataset item turns a real run into a case.
- **Experiments** (`AgentExperiment`, `AgentExperimentResult`) run each item, once per trial, through an agent (an
  `AgentRun` on the job surface as the user who starts it, through `ec.llm.startRun`: the agent must serve `job`
  and the user needs `AgentAdmin`) or a service (`input` and `metadata` in, `output` out). The job
  `advance_AgentExperiments_frequent` (each minute) reads the runs that ended, scores them with the scorers attached
  to the dataset, and ends the experiment with its mean score and pass rate. At most 200 items × trials. Compare
  shows two experiments of a dataset item by item, worst change first.
- **Live scoring**: a scorer attached to an agent with a sampling rate scores that share of its completed runs
  (chosen by a hash of the run ID), from `score_AgentLiveRuns_frequent` each five minutes.
- **Scores** (`AgentScore`) say where they came from: an experiment, a live sample, a run scored from Runs, or a
  person's review. A scorer that fails stores a score row with its error, and the other scorers and items go on.

Scores are database rows, not telemetry: they are not on the OTel spans, and an experiment's runs are not linked to
their traces.

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
- **The dataset screens** (Datasets, with the hidden DatasetDetail, DatasetFields and DatasetView) are the
  [business datasets](#business-datasets)' list and stewardship: the datasets the viewer reads, and a steward's waiting
  for review, each with its state, source, scope, index and last build; a new dataset over a registered feed's document;
  one dataset for its steward (identity and scope, admission checks, Admit, Rebuild index, Retire, the definitions that
  use it); its document's fields and conditions; and its rows as the viewer reads them, with their export as CSV or
  JSON. Below the business list, where no app mounts the screen, Datasets shows Tools' other datasets: the telemetry
  indexes, the DataDocument indexes and the saved datasets that are not business ones, each opened in Explore, a saved
  one in the definition editor, with the fields of the one chosen. Their presentation is `tools/template/ReportDatasets.js`,
  which reads no entity: every read and write is a `ReportServices` operation called as the viewer.
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
  view (in the TUI it closes the inspector first). Playlists rotate dashboards in kiosk mode for a wall display
  ([Playlists and the wall display](#playlists-and-the-wall-display)); a
  playlist of one dashboard loads it again at each turn, and `m-report-playlist` with `kiosk` (the `WallDisplay` page, which a shared device opens under BOSS Insights) offers no link out, retries a
  failed load at the next turn instead of pausing, and reloads the page (at most once a minute) when a load is refused
  (401, 403), so the server's plain refusal page replaces it, or when the playlist changed under it ([Shared devices](../security/Shared-Devices.md)).
  The time picker has quick ranges, the viewer's recent ranges and an absolute range. Each panel's menu has view,
  edit, share (a link with the time made absolute), explore, inspect, drill ([Drill](#drill)), duplicate, remove and
  new alert rule. A dashboard's owner shares it with a group from its Share dialog ([Sharing](#sharing)). The
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
  visitor who is not signed in (such as on a public site) gets nothing, and a definition the viewer may not read logs
  a warning and renders nothing. A definition the viewer may read over a dataset the viewer may not is drawn, each such
  panel saying "Not available: viewer may not read dataset …": a screen that embeds for readers who may lack the
  dataset checks first (`list#Datasets` leaves out what the user may not read) and leaves the embed out, as Tools'
  service and entity detail do.
- **Reusing the dashboard list.** `Dashboards/Home.xml`'s links run through its own transitions (`dashboard`, `star`,
  `edit`, `new`, `explore`), so a screen that includes it takes them in with `transition-include`, as
  `Dashboards/Panels.xml` and BOSS Insights › Dashboards do. With no mount a dashboard then opens in that screen's
  sibling `Dashboard` screen, and editing and Explore open the Dashboards app; under a mount each goes where the
  mount's `reportLinks` send it ([below](#mounting-the-report-screens): in BOSS, Insights › Dashboard, its editors and
  Report builder › Explore), and a link the mount does not offer is not shown.
- **Definitions.** Dashboards, library panels, datasets, saved searches, alert rules and report schedules are JSON in
  `moqui.report.Definition`, with versions (diff and restore) and references (the "used in" lists, export and
  import). Components ship them as `report/*.json`, one file each with `id`, `type`, `title` and `body`; shipped ones
  are read-only until saved as a copy. A shipped alert rule is a template: it runs only once copied, which gives it an
  owner. The owner can delete a saved definition (`delete#Definition`; Delete on the Definition, Alerts and shared item
  screens) with its versions and an alert rule's state; a shipped one cannot be deleted, nor one that other definitions
  still reference (the error names them). Deletes are logged to `moqui.report.audit`. A definition is its owner's, and
  is read by one group beside them when it is shared ([Sharing](#sharing)).
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
  a preview of several CSVs downloads as a zip. An empty report is skipped. Saving to a server location needs
  `REPORT_SAVE_LOCATION`. Dashboards › Reports previews and downloads a dashboard or report, opens the composer of a
  multi-dashboard report, links the playlists, and schedules a report in owner mode (`create#Schedule`: to one address
  line, a group or a server location, as its owner); the schedules themselves are on the delivery screens.
- **Analysis** builds an aggregate over a dataset from typed choices and saves it as a library panel
  ([Analysis](#analysis)). **Playlists** lists, makes and shares the dashboard playlists; one plays in the app
  (`Playlist`) or full screen and unattended (`WallDisplay`).
- **The delivery screens** (Scheduled reports, `Schedules`, with the hidden `ScheduledReportDetail` and `ReportDelivery`)
  are the viewer's side of [report delivery](#report-delivery). Schedules lists Mine (the schedules that go to the viewer
  alone, the owner-mode ones and the screen schedules they set up for themselves), Shared administration (the group
  schedules they own, for one who may deliver to others) and Received (the reports delivered to them, each with their
  own report rendered again), and makes a schedule: a dashboard or report the viewer reads, a timing and a format, for
  themselves or for people and groups, each checked by the engine. ScheduledReportDetail is one schedule for its owner:
  its timing and output, its recipients beside each one's result of the engine's check, the viewer's own preview, its
  runs, and Resume, Send now and Pause (an owner-mode schedule is paused, resumed and sent there, not changed).
  ReportDelivery is one run: what ran and each recipient's outcome for its owner (who retries the failed and
  reconciles the uncertain) and for the administrator (with its owner by name), a recipient's own delivery alone, and
  the same statuses as CSV. What a viewer is offered follows their own grant of each operation, so nothing is asked that
  would only be refused. Their presentation is `tools/template/ReportDelivery.js`, which reads no entity: every read and
  write is a `ReportServices` operation called as the viewer.
- **The alert screens** (Alerts, with the hidden `AlertRule` and `AlertShared`). Alerts is one list: All (every rule
  the viewer reads), Mine, Received (the rules that told them, with their own delivery) and Templates (the shipped
  rules, each copied to the viewer over a dataset they read), each rule with its kind (an operational rule tells
  contact points, a [business rule](#business-alerts) an audience), its condition, whom it tells and the most severe
  state of its series; pause, activate (which runs the rule's reading as its owner first) and delete, one rule or
  several; and below, the state of each series and the shared contact points, silences and mute timings. AlertRule is
  one rule, for anyone who reads it: the basis every rule has (its name, the query, the measure and its reduction, the
  condition and its timing), then whom it tells by its kind: an operational rule's contact points, or a business
  rule's audience (the owner alone, or people and groups, each with the result of the engine's check, in the app and
  at the account's address, and the group it is shared with) and each recipient's last delivery; either kind's
  silences, mute timings and shared items; and the state of each series as the viewer may read it. A rule's kind and
  state are the engine's (`get#AlertState`): no screen reads an alert instance. AlertShared is a shared contact point,
  silence or mute timing. Their presentation is `tools/template/ReportAlerts.js`, which reads no entity.

### Mounting the report screens

An app shows the Dashboards screens under its own root (BOSS Insights does: impl-insights P2,
[plans/Admin-Screens-Down.md](../../plans/Admin-Screens-Down.md) Track I), each in one of two ways:

- **By location**: a `subscreens-item` of the app names the Tools screen file. Every transition of the screen is then
  on the app's route, so this is for a screen with nothing that only an instance administrator may do, that no grant of
  the app names, and to which the app adds nothing.
- **Included**: a screen of the app's own includes the Tools screen (`include-screen share-scope="true"`) and takes in
  the transitions it wants (`transition-include`); a transition it does not take in does not exist on its route. The
  app's screen can then carry a grant of its own, set what depends on the request in its `always-actions`, and put its
  own parts around the Tools screen.

| Tools screen | How an app shows it | In BOSS Insights |
|---|---|---|
| Home (the list) | Included, with `definitionType` set; `dashboard`, `star`, `edit`, `new` and `explore` taken in | Dashboards |
| Dashboard | Included; `edit`, `setVariable`, `exportPanel`, `inspectPanel`, `duplicatePanel`, `removePanel` and `share` taken in; the app's screen sets `reportFixedVariables` | Dashboard, with the organization in scope and the measures' meaning around it; the Share dialog is the Tools screen's |
| Definition | Included, once for each editor; `save`, `copy`, `restore`, `delete`, `export`, `reference` and `dashboard` taken in (never `import`, unless the app mounts it); the app's screen sets `reportDefinitionTypes` and carries the authors' grant | DashboardEditor, PanelDetail |
| Panels | By location | Library panels |
| Explore | By location | Report builder › Explore |
| Split | By location | Report builder › Compare, hidden from the menu |
| Playlists | A screen of the app's own around its sections (`PlaylistsHeader`, the `Playlists` list, extended), with the page from `ReportPlaylists.js` `list()`; `createPlaylist`, `play`, `edit` and `sharePlaylist` taken in | Wall display (`Playlists`): the wall displays' share links, the devices showing each playlist and Register as display around the list |
| Playlist | Included; `data` taken in | the same screen, with a playlist chosen |
| WallDisplay | Included at the app's own location, with `data` taken in: a shared device's route group and its display account's grant name that location and that transition alone | WallDisplay, hidden from the menu |
| Drill | A screen of the app's own around its sections (`DrillUnavailable`, `DrillBreakdownRefused`, `DrillPanelBuckets`, `DrillBreakdownRows`, `DrillSourceRows`, the `Breakdown` form extended), with the page from the app's service over `ReportDrill.js` `view(request, options)`; it has no transition | Drill, hidden from the menu: the organization in scope, the cost filter, the measure's meaning and each record's Insights screen |
| Analysis | A screen of the app's own around its sections (`AnalysisSource`, `AnalysisRefused`, `AnalysisResult`), with the page and `saveAnalysis` from the app's services over `ReportAnalysis.js` (`fieldsOf`, `choicesOf`, `queryOf`, `resultOf`); Tools' `saveAnalysis` is not taken in | Report builder › Analysis, the author's and the steward's: the admitted datasets, the organization's calendar, Insights' reason codes |
| Datasets | Included; `createDataset` taken in (never `explore` and `definition`, the links of Tools' other datasets, which are not shown under a mount) | Report builder › Datasets, the author's and the steward's |
| DatasetDetail | Included; `storeDataset`, `admitDataset`, `retireDataset` and `rebuildIndex` taken in (never `definition`, the dataset in Tools' definition editor) | Report builder › DatasetDetail, the steward's |
| DatasetFields | Included; `storeFields` and `storeConditions` taken in | Report builder › DatasetFields, the steward's |
| DatasetView | Included; `exportDocuments` taken in | Report builder › DatasetView, the author's and the steward's |
| Schedules, ScheduledReportDetail, ReportDelivery | By location, the three under one parent whose `always-actions` add the recipient cap to the policy (`policyWith`, below) | Scheduled reports › Schedules (the default), ScheduledReportDetail and ReportDelivery, every Insights user's to open |
| Alerts | Included; `copyAlert`, `pauseAlerts` and `deleteAlert` taken in (never `evaluate` and `deleteShared`, Tools' own, unless the app's rules have contact points) | Business alerts › Rules (the default), every Insights user's to open |
| AlertRule | Included; `recipientChoices`, `storeAlert`, `storeAudience`, `pauseAlert`, `testAlert`, `copyAlert` and `deleteAlert` taken in (never `addContact`, `addSilence`, `addMute`, `removeItem`, `addRef`, `removeRef` and `evaluate`, unless the app's rules have contact points); under the same parent as the list, whose `always-actions` add the recipient cap | Business alerts › AlertDetail |
| AlertShared | Not shown by an app whose rules tell an audience | (none: `alertShared` is null) |

The dataset screens link each other as siblings (`../DatasetDetail`, `../DatasetView`, `../DatasetFields`,
`../Datasets`), so an app shows the four under one parent with these names; they open a dataset in Explore by the
mount's `explore` link, as a link of their own and not a transition, so an app takes in nothing for it. The delivery
screens link each other as siblings too (`../Schedules`, `../ScheduledReportDetail`, `../ReportDelivery`); every
transition of theirs is on the app's route, and each reads the mount's policy, never the request (below). The alert
screens link each other by the mount's `alerts`, `alertRule` and `alertShared` links, as links of their own and not
transitions, so an app's two screens need not be named as Tools' are (Insights: `Rules` and `AlertDetail`). The
playlist screens link by `playlists`, `playlist` and `wallDisplay`, and Drill and Analysis by `drill`, `analysis`,
`dashboard` and `panel`, the same way. A screen of the app's own around a Tools screen's sections (`section-include`,
`form-list extends`) sets what those sections read under the same names (`playlistView`, `drill`, `analysisView`),
from the Tools module or from its own service, and adds its own parts: a record's row links the screen the app gives
it (`recordUrl`), a list gains the app's columns. The other
Dashboards screens (Reports, Annotations, Git sync, Log context) are not shown by an app: Reports, Tools' own desk
with the owner-mode schedule form, is linked from Schedules only where there is no mount. The screens run under the app's authorization;
using them needs no `DASHBOARDS_APP` or `REPORT_DATASETS_ADMIN`, only the ReportServices operations a capability uses
and each dataset's own artifact.

The app's root sets, in its `always-actions` so that the transitions read them too:

| Context | What it does |
|---|---|
| `reportLinks` | Where each report link goes, as screen URLs: `dashboards` (the list), `dashboard`, `definition` (the editor), `panel` (the library panel editor), `explore`, `split`, `playlists` (the list of playlists), `playlist` (where one plays in the app), `wallDisplay` (the page one plays on full screen), `analysis`, `alertRule`, `logContext`, `import` (where definition import returns), `share` and `drill` (the drill screen, which each panel's menu offers as Drill, in the browser and the terminal: it opens the screen with `definitionId`, `panelId`, `from`, `thru` and `variablesJson`). Without a mount the screens keep Tools' own (`//apps/dashboards/...`; the list opens a dashboard in its sibling `Dashboard`); under a mount a key the map leaves out or sets to null is not offered: no editor hides New, Edit, the panel menu's Edit and `e`, and Explore's Save; no `split` or `logContext` hides Explore's Compare and Surrounding entries; no `import` hides import; no `drill` hides the panel menu's Drill, no `wallDisplay` Play full screen, no `analysis` Drill's Open as analysis; a `share` (the screen the app shares in; Tools names none, and shares on Dashboard and Playlists beside the editor's group field) hides the editor's group field and the editor's save keeps the saved group, whatever group is posted. `alertRule` is where a rule opens, for anyone who reads it (the alerts a user receives link it), so a mount names it for every user; a panel's New alert rule is offered there only where the policy lets the viewer set one (`features.alert`). `alerts` (the alert list) and `alertShared` (a shared contact point, silence or mute timing) are where the alert screens link each other |
| `reportPolicy` | What the app's policy is for the report screens it shows (the table below). Data only: a screen shown under a mount still asks its service, and the service decides. An area of the app adds to it for the screens below it by setting it again in its own `always-actions`, from `ReportMount.js`'s `policyWith(fields)` (the root's map with the area's fields): a field that costs a read the root should not make on every request, as the recipient cap of the area that delivers |
| `reportFixedVariables` | Dashboard variables the app fixes, such as the organization in scope: a viewer's `var-` parameter, carried value or panel export and inspect variables never replace one, and it is shown as a constant (set by the Dashboard screen's own `always-actions` when it depends on the request) |
| `reportHiddenComponents` | Components whose shipped definitions the dashboard list leaves out, and the schedule list does not offer or take for a new schedule (Insights: `tools`, the telemetry dashboards) |
| `reportDefinitionTypes` | Set by an editor's screen in its `always-actions`: the definition types it offers, and the only ones its save and copy take |

`reportPolicy`'s fields, each with what Tools' own screens do, which is also what a mount gets for a field it leaves
out, but for the features: a mount offers one only when its map says so.

| Field | What it is | With no mount | Read by |
|---|---|---|---|
| `features.alert` | Whether the viewer may set an alert rule here: new, copy, change, pause, activate and delete. Every such transition of the alert screens refuses without it, and none is offered | true | Alerts, AlertRule, AlertShared (every transition that writes); Dashboard (a panel's New alert rule) |
| `features.share` | Whether the viewer may share a dashboard or a playlist of their own here. The share transitions refuse without it, and no sharing is offered ([Sharing](#sharing)) | true | Dashboard (`share`), Playlists (`sharePlaylist`), Analysis (`saveAnalysis`, for a group named) |
| `range` | `{from, thru}`: the range a screen opens on when the request names none; the wall display's range, whatever its request names | `now-7d` to `now` | Dashboard (and its panel export and inspect), Explore (and its tail), DatasetView (and its export), Playlist (and its `data`), WallDisplay (and its `data`, always), Drill, Analysis (and `saveAnalysis`) |
| `datasetIdPrefix` | What the id of a dataset created here starts with, before the form's short name. The engine creates a dataset only under an id whose own update artifact the creator holds, so the prefix names what the app's stewards are granted and gives no right | none | Datasets (`createDataset`) |
| `datasetScopeLabel`, `datasetScopeHelp` | The words of the dataset forms' scope field; the `rowFilterParameters` entry it is sent in is the scope policy's own, from its registration ([Business datasets](#business-datasets)) | "Scope field", "The field that carries each row's scope; the policy's own when left empty" | Datasets, DatasetDetail |
| `recipientPermissionId` | The permission a person holds to be proposed and accepted as a recipient: the schedule and alert screens ask `find#ReportRecipients` with it for the choices and again, in the transition, for the people a save names, and refuse the save when one is not among its holders | the engine's own rule (an active internal account) | Schedules (`createSchedule`, `recipientChoices`), ScheduledReportDetail (`storeAudience`, `recipientChoices`), AlertRule (`storeAudience`, `recipientChoices`) |
| `reportEmailTemplateId` | The `EmailTemplate` every save of a report schedule here is sent with to `store#Schedule` | the engine's (`DASHBOARD_REPORT`, or the one the schedule has) | Schedules (`createSchedule`), ScheduledReportDetail (`storeSchedule`, `storeAudience`) |
| `alertAudience` | `{topic, emailTemplateId, detailLink}`: the fixed parts of an alert rule told to an audience of users and groups. A mount that names it tells every rule set there to an audience: each save writes the three from the policy (the template where email is a channel) and drops any contact point; a rule there watches a panel's saved query, never one typed; only business rules are listed and opened; and contact points, silences, mute timings, shared items and Evaluate now are not shown | null: a rule's kind is the engine's, an operational rule has contact points and a business rule the audience it is given, on the engine's own topic | Alerts, AlertRule (every save, copy and activation) |
| `shareGroups` | `[{value, label}]`: groups a definition is offered to be shared with beside the viewer's own, by the app's words for them (its wall displays). Being here gives no right: such a group is offered, and taken by `save#Definition`, only for a viewer whose own grant holds its share artifact ([Sharing](#sharing)) | none | Dashboard (`share`), Playlists (`sharePlaylist`) |
| `recipientCap` | The most recipients a group delivery is saved and checked with, sent with every save to `store#Schedule`, which refuses a larger audience and holds the cap under the instance ceiling; and written into the audience of an alert rule at every save, which `save#Definition` checks the same way. Added by the area that delivers (`policyWith`), not by the root | the engine's (50, under the ceiling) | Schedules (`createSchedule`), ScheduledReportDetail (`storeSchedule`, `storeAudience`, and the audience shown), Alerts and AlertRule (every save of a rule told to an audience, and the audience shown) |
| `calendar` | `{timeZone, weekOffset}`: the zone a time grain's buckets are cut in and a week bucket's offset (the engine's `time_zone` and `offset`), in the query Analysis builds, runs and saves. The root's is that of the scope it knows; a screen of the app's own with a scope choice of its own passes its own (Insights' Analysis: the chosen organization's) | the viewer's zone, the instance's week | Analysis (the result and `saveAnalysis`) |

`tools/template/ReportMount.js` is the one reader of the two maps: `state()` gives `{mounted, links, policy}` in a
screen's actions, every link key present (null when not offered) and every policy field with its default, and
`url(key)` is a transition's response, the mount's screen or Tools' own. `policyWith(fields)` is the root's policy with
an area's fields added, for the area to set as `reportPolicy`; with no mount it is null, so an area adds to a mount and
never makes one. A screen never reads `reportLinks` or `reportPolicy` from the context itself. A value that is not a map
is a mount with nothing in it: no link is offered and no feature.

**The policy is not authority, and a request cannot stand in for it.** The roots set or null the maps in
`always-actions`, which run before every transition too, so what a screen or a transition reads through the module is
the mount's, whatever a request sends. Where a field says what a save may hold, the transition that saves reads it
there and never takes it from its parameters, and that transition is the only way to the operation for the app's
users: a service is reached from outside the server only through a screen's transition, a REST resource that names it
(`tools/service/moqui.rest.xml` names the query, document and dataset reads and the annotation write, no schedule
operation) or an agent tool row that opts it in (none does); an app's users hold no grant on Tools' own routes; and an
app that adds a screen, a resource or a tool over one of these operations owes the same reading of its policy. So for the delivery screens: the recipients a schedule names are accepted by
`find#ReportRecipients` under the mount's permission in `createSchedule` and `storeAudience`, before `store#Schedule`,
which then checks each one's scope; the template and the recipient cap are put on the request by the same
transitions and by `storeSchedule`, a posted `emailTemplateId` or `recipientCap` being no parameter of theirs; and a
definition of a hidden component is refused by `createSchedule`. For the alert screens: every transition that sets a
rule (`storeAlert`, `storeAudience`, `pauseAlert` and `pauseAlerts`, `copyAlert`, `deleteAlert`, `testAlert`, and
Tools' own) first asks the policy's `features.alert`, so a user whose grant opens the engine's `save#Definition` for
another purpose (in BOSS, a dataset steward) sets no alert through the app; under a mount that names `alertAudience`
the same transitions write the audience's topic, template and link and the recipient cap into the rule at every save
and take no query, contact point, topic, link or cap from the request; and the people and groups an audience names,
and the group a rule is shared with, are accepted by `find#ReportRecipients` under the mount's permission in
`storeAudience`, before `save#Definition`, which then checks each one's scope for an active rule. For sharing:
Dashboard's `share` and Playlists' `sharePlaylist` first ask the policy's `features.share`, so a user whose grant
opens the engine's `save#Definition` for another purpose (in BOSS, a steward or a distributor) shares nothing through
the app; the group posted must be one the transition offers (the viewer's own, or one of the policy's `shareGroups`
that the viewer may name), and `save#Definition` then asks the same of the group itself, whatever the policy lists:
a group the saver belongs to, or one whose share artifact their own grant holds. For the wall display: the range of
its page and of its `data` is the policy's, and `from` and `thru` are no parameters that are read. For Analysis: the
calendar of a time grain's buckets is written into the query by the screen's own actions and by `saveAnalysis`, and no
parameter names it. What does not depend
on the mount is the engine's own, checked whatever calls it: the owner, the group a definition is shared with, the
group delivery artifact for anyone but oneself, each recipient's scope fingerprint at the save, at a resume and before
each message, and the cap under the instance ceiling.

Dashboard sends the links to the browser and the terminal as the dashboard's `appLinks`, server paths beside
`screenUrl` (`definition`, `panel`, `explore`, `alertRule`, `dashboard`, and `drill` where there is a drill screen), and
Explore's count over time carries its own `explore`; the panel menu, the `e` key, Explore from a panel, a filter outside
a dashboard and the duplicate and remove posts use them, and a dashboard without them uses Tools' screens. Under a mount
the editor's dataset choices are the datasets the author may read (`list#Datasets`, each dataset's artifact), a new
panel starts on the first of them, and Explore and Split open on one of them. A viewer who reads no dataset, and asks
for none, is told so by Explore and Split, which then query nothing (under Tools too), where they would open on a
telemetry dataset and show its refusal. A panel the viewer may not read (a dataset refused, a failed query) says "Not
available" with the reason, never "No data". Saving a definition still checks every dataset it references as the
author, so a copy of a dashboard over a dataset the author may not read is refused until the dataset is granted.

Request parameters are names in a screen's context, so these names are set only by screens: Tools' Dashboards and
Monitor roots set all five to null, and a mount sets each in `always-actions`, its root's or those of the screen that
reads it.

The mount's root grant is inherited by the screens it mounts (impl-admin F34): deny, on the screens of an area only some
groups use, the root's groups and grant the others there, and take in only the transitions the area needs. An
inheritable `AUTHZT_ALWAYS` on a screen passes every deny below it, including the engine operations' own, so grant such
an area with `inheritAuthz="N"` on the screen and on its transitions by name (`AT_XML_SCREEN_TRANS`, `<screen>/<transition>`,
as BOSS Insights' editors): a transition not named is refused, and each engine operation is checked by its own grant. A
grant names the app's own screen, never a Tools location, which would open the screen in the Dashboards app too. The
regression spec is `tools/test/ReportMounts.spec.js`, over the test mount `tools/test/screen/ReportMount.xml`: the
links, the policy and its defaults under the mount, with no mount and with a forged request parameter, and, read from
the screens' sources, that both Tools roots null the five names and that no screen reads the two maps but through the
module. The dataset screens under a mount (the id prefix, the scope field's parameter and words, the transitions an app
does not take in) are held by the app's own spec through its route (mr-boss `test/Insights/InsightsDatasets.spec.js`),
and with no mount by the cargo tests that render them (`tools_screens.rs`, `monitor_tui.rs`). The delivery screens are
held the same way: under a mount by mr-boss `test/Insights/InsightsDelivery.spec.js` (the recipients kept to the
permission's holders, the template and the cap of every save whatever is posted, a hidden component's dashboard and
Tools' owner-mode form refused or absent on the app's route and taken on Tools' own), with no mount by the cargo tests,
and the operations they read and write by `framework/test/ReportAudience.spec.js`. The alert screens likewise: under a
mount by mr-boss `test/Insights/InsightsAlerts.spec.js` (a save refused to a user without the alert feature who holds
the engine's save, and taken for one with it; the topic, link and cap of every save whatever is posted; an audience
kept to the permission's holders, the viewer's own groups and the cap; Tools' contact point, silence, mute timing,
shared item and evaluation transitions absent on the app's route and taken on Tools' own), with no mount by the cargo
tests, and the engine's rules by `framework/test/BusinessAlerts.spec.js`. Sharing, the playlists, the wall display,
Drill and Analysis likewise: the share feature and the groups a share takes by `tools/test/ReportMounts.spec.js`
(under the test mount) and by mr-boss `test/Insights/InsightsDashboards.spec.js` and `InsightsWallDisplay.spec.js`
(a distributor's share refused and an author's taken; the wall displays' group taken from the author alone; the
device's route, which opens the app's wall display and nothing of Tools'), Insights' drill and analysis around the
Tools sections by `InsightsRecords.spec.js`, the screens with no mount by the cargo tests, and the group rule of
`save#Definition` and `find#ShareGroups` by `framework/test/ReportAudience.spec.js`.

### Business datasets

A dataset definition whose body says `"signal": "business"` (over a DataDocument, `documentType`) is a business dataset, kept
by `crates/moqui-report/src/business.rs` (BOSS impl-insights P3, INS-03):

- **Admission.** It is read only once admitted (a `moqui.report.BusinessDataset` row with `admittedDate` and no `retiredDate`),
  except by its steward, who holds the dataset's update artifact (`org.moqui.report.dataset.<id>`, action update; `ADMIN` holds
  all through `REPORT_DATASETS_ADMIN`). Until then a reader's query is refused "dataset … is not admitted", so a panel over it says
  "Not available", never "No data". `store#BusinessDataset` with `admit` admits it only when three checks pass: its document is in
  a registered DataFeed (a `DataFeedDocument` row), the document's index is built (and not older than its last field or condition
  change), and its `rowFilterService` is a registered scope policy. An admitted dataset whose definition no longer binds a
  registered policy is read only by its steward again, and so is one whose body names an `indexName` or a `serviceName` (a
  business dataset reads its one document). `find#BusinessDatasets` reports each dataset's state
  (admitted, review, retired, unavailable) and checks live; `validate#BusinessDataset` gives one dataset's checks to its steward.
- **Scope policies** are services registered as `ReportScopePolicy` enumerations (`enumCode` the service): the framework's
  `ReportUserGroups` (`get#UserGroupRowFilter`, rows whose field is one of the viewer's groups) and Mantle's `MantleOrganization`
  (`mantle.party.PartyServices.get#OrganizationRowFilter`, the viewer's organizations). A `rowFilterService` is called with
  `userId`, the dataset's id and the dataset's `rowFilterParameters` (such as `{"organizationField": "vendorPartyId"}`), so one
  policy serves every dataset by naming its field; a registered policy runs without a grant of its own (it is the engine's,
  reviewed code), any other with the viewer's. Its filter is added before documents, suggestions, value counts, aggregations,
  panel exports and playlists alike, so a dashboard's variables or a query's `ignoreVariables` cannot widen it. A
  registration also names the policy's scope parameter, the `rowFilterParameters` entry in which a dataset names the field
  that carries a row's scope, as the enumeration's `optionValue` (`groupField` for `ReportUserGroups`): the engine answers it
  as each dataset's `scopeParameter`, and the dataset forms send their one scope field under it, whichever policy is
  chosen. A policy that registers none is asked for no field.
- **Stewardship.** `create#BusinessDataset` (a dataset over a registered feed's document with a registered policy, owned by the
  steward, waiting for review; its creator holds the dataset-save artifact `org.moqui.report.dataset` and the update artifact
  of the dataset named, so a steward granted the datasets of a name pattern creates only under it, whatever id a caller
  sends; a saved dataset's body changed any other way, by `save#Definition`, a copy, a restore, an import or
  a git pull, returns it to review, and deleting the definition deletes its admission), `store#BusinessDataset` (title, time field, policy and parameters of a saved dataset, which return
  it to review; a shipped dataset's definition is its component's; steward; admit; retire; `expectedVersion`),
  `store#BusinessDatasetFields` (plain field paths only, no expression, secret or encrypted entity field; unique names; removing or renaming a field
  any definition over the dataset names is refused with those definitions' names) and `store#BusinessDatasetConditions` (a field of
  the document and an operator of the list, never a post-query expression). `index#BusinessDataset` rebuilds the dataset's
  document of its feed only (`index#DataFeedDocuments` with the document), for its steward, recording the time and count. An
  application grants its readers each dataset's artifact (view) and its documents' `DataDocumentUserGroup` rows; admission does
  not grant.
- **The document is read through its dataset.** A dataset that is not a business one (a DataDocument by its id, an `index:`
  alias, a definition without the signal) over a document some business dataset reads is opened only to a holder of that
  dataset's update artifact who also holds the update artifact of the document by its own id
  (`org.moqui.report.dataset.<document>`; `ADMIN`), so a view grant on such an artifact, by pattern or by mistake, reaches no
  unfiltered rows, and neither does a steward's own dataset: a steward granted the datasets they name by a pattern reads a
  business document through a business dataset and its registered policy only ([Dataset aliases](#dataset-aliases)).
- **What the screens read** beside these is two operations in `framework/service/org/moqui/report/ReportReads.js`, so that
  a screen an app shows under its own root reads no entity. `get#BusinessDataset` answers one dataset as
  `find#BusinessDatasets` does, to a caller who reads or stewards it (any other dataset, and one that does not exist, is
  "not available"), and to its steward also its document's `fields` and `conditions` and the definitions over it that the
  caller reads (`uses`; `usesAvailable` false when they could not be listed); a reader gets the dataset alone.
  `get#BusinessDatasetOptions` answers what a dataset form offers: the registered `policies` with their `scopeParameter`
  and the `operators` and `functions` a steward may use, to any signed-in caller, and `canCreate` (the dataset-save
  artifact by the caller's own grant) with, only then, the `documents` of registered feeds (never a telemetry index). It
  takes nothing from the request. Both are granted with the other `ReportServices` operations (`DASHBOARDS_APP`'s
  pattern); an app grants them to the groups that list datasets.
- The regression spec is `framework/test/BusinessDatasets.spec.js`.

### Dataset aliases

A dataset definition may read a source that has a dataset artifact of its own, under a filter the definition chooses: a
telemetry index (`"indexName": "otel_logs"`, or the index named as its `documentType`), or a business dataset's document
without the business signal. The alias's own artifact says nothing of the source, and an application may grant it by a name
pattern to whoever saves datasets, so the engine asks for the source's own artifact (`crates/moqui-report/src/alias.rs`), when
the definition is saved (`save#Definition`, a copy, a restore, an import, a git pull) and each time it is read:

- **A telemetry index** (`org.moqui.report.dataset.otel_logs`, view; `SYSTEM_APP` and `REPORT_DATASETS_ADMIN` hold them).
  The saver must hold it. A reader reads the alias when they hold it themselves, or when the definition's owner does: the
  owner is asked at each read, in a context of their own, so an alias an administrator saves and shares is a narrowed view
  for readers who do not hold the index (a registered annotation source's dataset, for one), and it stops answering when its
  owner loses the grant, when the owner's account is closed (disabled for good or past its end date, as their session and
  schedules stop), or when the definition is handed to an owner without the grant. `transfer#DefinitionOwner` hands an alias
  on only while the owner it leaves holds the index, so one that has stopped answering is not opened again by the grant of
  whoever takes it; left behind, it answers a holder of the index only. A definition a component ships
  (`report/*.json`) is that component's reviewed code and is read by its own artifact, as mr-mantle's
  `mantle-business-events` is. An alias still needs a row filter in its definition, and a dataset has no field list: a
  reader of the alias reads each matching line whole.
- **A business dataset's document** (`org.moqui.report.dataset.<document>`, update): the saver and every reader, shipped
  or saved, beside the alias's own update artifact (above). No owner's holding opens it to others: a business document is
  read by others through an admitted dataset. A definition that says `signal: "business"` with no registered scope policy as
  its `rowFilterService` is held to this too, saved and read: the signal alone does not let its steward read the document
  unfiltered before admission.

A row written past the save (an entity write, an older row) is held to the same rule when read. The rule's unit tests are
`alias_tests.rs`; the refusals, saved and stored, are a feature of `framework/test/BusinessDatasets.spec.js`.

### Semantic guards

What a business dataset's `fieldDefaults` declare about a measure decides which aggregations of it mean something; the engine
refuses the others with a coded reason, for every caller (a panel, Explore, `run#Query`, an alert, a schedule, a drill). A
dataset that is not a business one is read as before.

| Declared on a field | Refused | Until |
|---|---|---|
| `unitField` (an amount's currency, a quantity's unit) | `UNIT_MIXED`: a sum, average, least or greatest over records of several units (it was shown without a unit) | the query groups by the unit field (a terms aggregation of it above the amount, not beside it), or is filtered to one unit; a count of the field is in no unit; a measure the owner converts to one unit has a fixed `unit` and no `unitField`. Rows grouped by the unit are not added again: `subtotals`, a footer total (`totals`), or a `reduce`, `groupBy` or `pivot` step that adds across the unit is `UNIT_MIXED` too |
| `ratio: {numerator, denominator}` (a stored rate) | `RATIO_AGGREGATE`: a sum or an average of it | the group's rate is computed from the summed numerator and denominator (a calculate step) |
| `nonAdditive: [dimension, ...]` (a balance or snapshot) | `NONADDITIVE`: a sum across one of the dimensions | each is a group of the query, or pinned to one value by a filter |

A document query returns a page: when more records match than were read, the result carries a `bounded_result` notice with
`returned` and `total` (`sourceCount` stays the engine's own count of what matches), and a total, a `reduce`, a `groupBy` or a
`pivot` of those rows is refused (`BOUNDED_RESULT`).

A dataset whose definition says `rows: "restricted"` gives its readers aggregates only: its records (`query#Documents`, a
query of mode `documents`, a top-hits aggregation, a drill's records) need the artifact `org.moqui.report.rows.<id>` (view),
and are refused with `ROWS_RESTRICTED` otherwise. The rules are in `crates/moqui-report/src/semantics.rs`; the regression spec
is `framework/test/ReportDrill.spec.js`.

### Drill

`ReportServices.get#Drill` goes from a value of a saved panel to a breakdown and the records behind it. The request names a
definition the viewer reads (`definitionId`: a dashboard with `panelId`, or a library panel), one of the panel's queries
(`queryReference`, its index, 0 by default), the `range` (or `from`, `thru`), the dashboard's `variables`, the buckets chosen
(`selectedBuckets`: `field` and `value` as the panel showed them), an optional `breakdownDimension` and a page. It never takes a
query, a dataset, an entity or a service.

- The engine reads the definition as the viewer and runs the panel as its dashboard does: variables fill the saved variables
  only, and each dataset's artifact, admission and row filter apply again.
- Each chosen bucket's predicate is taken from the panel's own result, where the engine recorded it, so an "Other" bucket is
  everything but the buckets shown and a missing bucket the records without the field. A bucket that is not in the result is
  refused.
- The answer is the saved query narrowed by those predicates: `selected`, `panelFrames` (the panel's result, whose buckets may
  be chosen), `dimensions` (the dataset's aggregatable fields), `breakdown` (the saved measures by one of them, or `refused` with
  a guard's reason), and `rows` with `columns` (the query's time, groups and measured fields), `total` and `bounded` (a page is
  a sample). A page holds at most 100 records and the pages reach the first 10,000; past that the viewer chooses a bucket or
  a shorter range.
- A viewer who reads the dataset's aggregates and not its records gets `rowsAvailable` false with `rowsReason` and no record
  (nothing is fetched). A panel the viewer cannot read, or whose value is not an aggregate of a dataset, answers `available`
  false with its reason.

Dashboards › Drill (`tools/screen/Dashboards/Drill.xml`, hidden from the menu, with no transition) is its page, opened
from each panel's menu: the panel's buckets to choose from, a breakdown by one more of the dataset's dimensions (or
why the guards refuse it), and a page of the records with the query's own columns, a sample when more match; back to
the dashboard, and on to Analysis over the same dataset. `tools/template/ReportDrill.js` `view(request, options)` asks
the engine as the viewer and presents its answer; under a mount the default range and the fixed variables are the
mount's (`reportPolicy.range`, `reportFixedVariables`). An app with business parts of its own shows the screen's
sections in a screen of its own and calls `view` from its service with its parts as `options`, each decided by the
service and never by a request: `fixedVariables` (its scope), `range`, `permitted` (which fields a viewer may see as
a column or a dimension), `row` (what it adds to a record's row: `recordUrl` and `recordParameters`, the app's screen
for the record) and `carry` (further parameters its links keep). A mount that names no `drill` link offers none. The
code is `crates/moqui-report/src/drill.rs`; the regression spec is `framework/test/ReportDrill.spec.js`.

### Analysis

Dashboards › Analysis (`tools/screen/Dashboards/Analysis.xml`, `tools/template/ReportAnalysis.js`) builds an aggregate
over a dataset the viewer reads from typed choices: the dataset (`list#Datasets`), how a measure is aggregated (count,
sum, average, least, greatest, distinct values) and of which number field, up to two groups, a time grain (day to
year), one equality filter and the range. No query language is added: the choices become the engine's own query, each
field checked against the dataset's fields (`get#Dataset`, with what the dataset's definition declares of each: its
unit field, a ratio, the dimensions it is not added across), and `run#Query` runs it as the viewer. The result shows
each bucket with its measures and units, the engine's count of the records behind them (never the number of rows
shown) and its notices; what the [semantic guards](#semantic-guards) refuse is shown as its reason ("Not a meaningful
result"), and nothing is computed around it. Save analysis (`saveAnalysis`) runs the choices again and saves them as
a library panel the viewer owns (`save#Definition`: an ordinary panel, which the definition editor then refines),
private or shared with a group offered to them ([Sharing](#sharing)), and optionally placed by reference on a dashboard
of their own; it is offered to a viewer whose own grant opens the save where there is a panel editor. Under a mount
the default range and the calendar of a time grain's buckets are the mount's (`reportPolicy.range`, `calendar`), read
by the screen and by the transition, never from a request. An app with business parts of its own (which datasets are
offered, who may build one, a scope choice with its own calendar, reason codes) shows the screen's sections in a
screen of its own, with the page and the save from its own services built on the module's `fieldsOf`, `choicesOf`,
`queryOf(choices, source, bucketParameters, deny)` and `resultOf`.

### Sharing

A definition has one owner and at most one group (`userGroupId`): the group reads the definition, and nothing else.
Each of its members still reads a dataset by their own grant and row filter, so a share never gives data.

- **The group rule is `save#Definition`'s**, whatever saves (a screen, a copy, a restore, an import, a pull): the group
  is one the saver belongs to by a membership of their own (never the implicit `ALL_USERS`), or one whose share
  artifact, `org.moqui.report.share.<userGroupId>` (update), the saver's own authorization records hold, with nothing
  inherited from the screen or service that asks. Any other group is refused (`SHARE_GROUP_UNAVAILABLE`). A group the
  definition already has is kept by a later save without being asked again, so an owner who left a group still saves
  their definition. `ADMIN` holds `org.moqui.report.share.*` (`REPORT_SHARE_ADMIN`); an app grants the artifact of a
  group no person is in to those who share with it (BOSS Insights: its wall displays' group, to its authors).
- **`find#ShareGroups`** answers the groups the caller may share with, by name: their own (`own` true), and of the
  `groupIds` named those they may share with by the artifact; a group named that is neither, or does not exist, is in
  `refusedGroupIds`. It grants nothing.
- **On the screens** (`tools/template/ReportShare.js`): Dashboard's Share dialog (`share`) and Playlists' Share a
  playlist (`sharePlaylist`) are offered to the owner of a saved dashboard or playlist: one of the groups
  `find#ShareGroups` answers (their own, and the mount's `reportPolicy.shareGroups` they may name, by the mount's
  words), or none to make it private again. The transitions hold the mount's `features.share` first and refuse a
  group they do not offer; the engine's save then checks the owner, the version the dialog read and the group. With no
  mount a viewer shares with a group of their own there, and the definition editor has its group field too, where
  `ADMIN` names any group; under a mount that names a `share` link the editor keeps a definition's group as it is.
  Analysis' save shares its new panel the same way.

### Playlists and the wall display

A playlist is a report definition of kind `playlist`: one to fifty dashboards in an order, each shown for five to 3600
seconds. Three screens (`tools/template/ReportPlaylists.js`):

- **Playlists** (in the menu) lists the playlists the viewer reads, each with its dashboards, its seconds and the group
  it is shared with; makes one (`createPlaylist`: dashboards the viewer reads, saved as their own; it is ordered in the
  definition editor), for a viewer whose own grant opens the save where there is an editor; and shares one
  ([Sharing](#sharing)). A playlist opens in Playlist or, by Play full screen, on WallDisplay in a window of its own.
- **Playlist** plays one in the app, for whoever sits at it: the dashboards in turn in kiosk mode over the request's
  range (the mount's when it names none), each also opened on its own. Its `data` transition answers the dashboard at
  an index of the stored playlist, rendered as the viewer.
- **WallDisplay** plays one unattended, on a page of its own and not the app shell: standalone, its html is a bare
  document (no navigation, menu, account or instance header) that loads the client's files and mounts the screen's
  qvt render as an island, `m-report-playlist` in kiosk mode. What an unattended browser may not choose is not taken
  from its request: the range is `reportPolicy.range` (Tools' seven days with no mount), never `from` and `thru`; only
  a dashboard of the stored playlist is rendered, by its index; a panel's failure is shown as "not available" with
  its text in the log, with no query inspection; any failure answers one fixed sentence; the screen has no transition
  but `data`, and nothing writes. Everything runs as the viewer: a dashboard they may not read is left out and logged
  by name.

A wall display on a [shared device](../security/Shared-Devices.md) is an app's: the app includes WallDisplay in a
screen at its own location with `data` taken in, and the device's route group and its display account's grant name
that location and that transition alone, so the Tools file opens nothing for a device and nothing else of the app
does either; the framework puts the registered playlist over whatever the browser asks for; and the app's root sets
the range for a device's request. The display account reads a playlist and its dashboards when they are shared with
a group it is in, which no person is in: its owner shares them there by the group's share artifact
(BOSS Insights: `BOSS_INSIGHTS_DISPLAY`, the mount's `shareGroups`, the route group `INSIGHTS_PLAYLISTS` over
`Insights/WallDisplay`).

### Report delivery

A schedule renders as its owner. A **business schedule** sends that result to another user only when it is what that user
would see, and to each recipient in a message of their own. A schedule is a business one when `store#Schedule` saved it (its
`deliveryMode` is `personal` or `group`) or when its definition reads a business dataset or any other dataset whose rows
depend on the viewer (one with a `rowFilterService` or `contextFilters`, such as a business events dataset over the
annotations), whatever wrote its row: one made by `create#Schedule`, or by an entity write, is held to the same check at each
run. A technical schedule (telemetry and other datasets that give every reader the same rows) is delivered as before, in
owner mode, to one address line.

- **The scope fingerprint.** A user's fingerprint for a definition is a hash of what they read of the definition and of
  everything it references (each dashboard and library panel, read or not), the documents of each dataset they may read, each
  dataset's resolved row filter (`rowFilter`, `contextFilters` and the `rowFilterService`'s answer, in a form that does not
  depend on the order of its values) and, for a dataset whose records are restricted, whether they hold them. What a
  dashboard reads without naming it counts too: the telemetry dataset of a query that names none, the change markers
  (`monitor_annotations`) and, over a business dataset, each registered source of business events. It is computed in
  a context of its own signed in as that user, so a screen's inheritable grant or a disabled authorization never enters it.
  Two users are owner-equivalent when their fingerprints are equal. A policy that names the viewer's own groups
  (`ReportUserGroups`) gives users of different groups different filters, so different fingerprints; a policy that answers the
  organizations a user works in gives every user of those organizations the same one. A definition over a service source is
  not comparable (the service decides its scope in its own code), so it goes to its owner alone.
- **`check#ReportAudience`** answers, for a definition the viewer would own a schedule of, each proposed recipient's result:
  the named users and the members in date of the named groups, each `eligible` or not with a `reasonCode` and a `reason` in
  words: `not-shared` (the definition, or a dashboard or panel it uses, is not theirs to read: its owner shares it),
  `scope-differs`, `not-comparable`, `denied-source` (the owner does not read a source), `inactive`, `external`,
  `no-address`. A reason never says what the other user holds. Recipients other than the viewer need the artifact
  `org.moqui.report.delivery.group` (`REPORT_DELIVERY_ADMIN`, `ADMIN`'s; an app grants it to its distributors).
- **`store#Schedule`** creates or changes a business schedule its viewer owns, delivered by email only (no save location). The
  server sets the mode: `personal` when the owner is the only recipient, `group` otherwise, with its recipients in
  `moqui.report.ReportScheduleRecipient`. An active group schedule is saved only when every recipient passes
  (`RECIPIENT_REFUSED`, naming each with its reason code; a group's member without an address or an active account is not a
  refusal, they are skipped at each run) and the audience is within the cap (`RECIPIENT_CAP`). The cap is `recipientCap`, 50
  without one, held under the instance preference `report.groupRecipientCeiling` (Admin's, 200 by default). A change needs
  the revision read (`expectedRevision`, `STALE_REVISION`). `pause#Schedule` resumes a group schedule only when its recipients
  pass again.
- **A run** opens a `moqui.report.ReportOccurrence` (a cron occurrence keyed by the schedule and the cron instant, or a Send
  now), pins the owner, the schedule's revision and the definition's version, and for each recipient, just before their
  message: checks the lease, the owner and the schedule again, computes the recipient's fingerprint again, and sends one
  message to their one address, or records why not. Each recipient has a `moqui.report.ReportRecipientDelivery` row: `sent`
  with its message id, `skipped` with a safe reason (`scope-differs`, `inactive`, `no-address`, `over-cap`,
  `owner-unauthorized`, `delivery-blocked`, `schedule-changed`, `removed-audience`), `failed` (`transport-failure`,
  `source-failure`) or `uncertain` (the transport does not know whether the message was accepted). One recipient's failure
  does not stop the others. An occurrence that runs again (a run that ended before its schedule's last run time was
  kept) sends to nobody whose delivery is `sent` or `uncertain`. One render serves every recipient of a locale (`renderCount`): they share the definition's
  version and the fingerprint. A data export's message carries its files; a PDF's or a picture's also the dashboard inline.
- **No row keeps a report.** An occurrence and its deliveries are evidence: who, when, which revision and version, what state
  and which reason code. `find#ReportOccurrences` lists the viewer's own schedules' occurrences as status and counters; with
  `received` the deliveries to the viewer; with `administrative`, for a holder of `ADMIN_TRANSFER_VIEW`, every owner's, without
  titles. `get#ReportOccurrence` answers each recipient's outcome to the schedule's owner (and to that administrator), and to
  a recipient their own delivery alone. `get#RecipientReport` renders a delivered report again as its recipient, and only
  while their fingerprint is still the one the delivery was sent under (`scope-changed` otherwise); another user's delivery
  id is refused as one that does not exist.
- **Retry.** `retry#ReportRecipients` sends again to the failed deliveries named, for the schedule's owner, at the
  occurrence's revision read, with each recipient checked again; a sent delivery is never replayed, and a schedule or
  definition changed since the occurrence is refused (`DEFINITION_CHANGED`: send a new occurrence). An uncertain delivery is
  never retried blind: `reconcile#ReportRecipient` records what a person found (sent, or failed and so retryable).
- **Ownership.** A business schedule whose owner is disabled or gone pauses (`pausedReason` `owner-disabled`) at its next
  run. `find#OwnedSchedules`, `pause#OwnedSchedules` and `transfer#DefinitionOwner` carry the owned-resource contract of
  `org.moqui.admin.IdentityServices` for an account's lifecycle: the user's own, or another user's for a holder of
  `REPORT_OWNER_CHANGE` (`ADMIN`'s). A transfer hands one schedule, with its definition and the definitions that references
  which the leaving owner owns, to an active internal user who may store a schedule, reads every source and, for a group
  schedule, may deliver to a group (`NEW_OWNER_REFUSED` otherwise). The schedule stays paused (`transfer-pending`) until the
  new owner resumes it, which checks the recipients against the new owner's fingerprint; nothing is sent meanwhile, by Send
  now or a retry either. The caller of these gains no report data.
- **Screen schedules** (a `ScreenScheduled` with a `screenPath` and no report definition) render as their user. One that
  names a `userGroupId` is sent to the group only when its screen path is registered as a `ReportLegacyAdapter` enumeration
  (`enumCode` the path), then one message a member, and only to members the saved find is shared with; without a
  registration the group is not sent to and the user's own delivery continues.
- **What the screens read and write** beside these (`framework/service/org/moqui/report/ReportReads.js` and
  `ReportWrites.js`), so that the [delivery screens](#the-dashboards-app), which an app shows under its own root, touch no
  entity:
  - `find#ReportRecipients` answers who may be proposed and accepted as a recipient, to a caller who may deliver to
    others (`canDeliver`: the group delivery artifact by the caller's own grant): a page of the active internal accounts
    by name (`users`: `userId`, `username`, `name`; `term` searches the full name and the username, ignoring case), the
    caller's own groups (`groups`, never `ALL_USERS`: a delivery goes to a group its owner belongs to) and, of the
    `userIds` and `groupIds` named, which may be recipients (`selectedUsers`) and which may not (`refusedUserIds`,
    `refusedGroupIds`). With `permissionId` only the holders of that permission are proposed and accepted, which is how
    an app keeps its reports to its own readers (`reportPolicy.recipientPermissionId`). The caller is always accepted;
    a caller who may not deliver to others is answered no account and no group and every other name refused. Being
    accepted grants nothing: the engine checks each recipient's scope.
  - `find#ScreenSchedules` answers the screen schedules the caller set up for themselves (the caller as `userId`, no
    report definition), each with its saved find's description and whether the group it names is sent to
    (`groupAdmitted`); `store#ScreenSchedule` changes such a schedule's timing, format, skipping when empty, active
    interval, subject (plain text: one that holds an expression is refused, since the subject is expanded when the
    schedule runs) and pause, and never its screen, find, user, group, template or location. Another user's schedule,
    a report schedule and one that does not exist are refused alike (`SCHEDULE_UNAVAILABLE`).
  - `get#ReportOccurrence` answers the run's owner by name too (`ownerName`), with the whole run: to its owner and to
    the administrator.

  They are granted with the other `ReportServices` operations (`DASHBOARDS_APP`'s pattern); an app grants the three
  new ones to the groups that schedule (BOSS: `BOSS_INS_REPORT_OWN`).

The code is `crates/moqui-report/src/delivery.rs` (and `schedule.rs` for the run); the regression spec is
`framework/test/ReportAudience.spec.js`, which also holds what the three screen operations answer and refuse.

### Business alerts

An alert rule is evaluated once, as its owner. A **business rule** tells that one state and its values to another user only
when it is what that user would have read: when their scope fingerprint for the rule equals the owner's, the check of
[Report delivery](#report-delivery). A rule is a business one when its body names an `audience`, or when its query reads a
business dataset or any other dataset whose rows depend on the viewer (a `rowFilterService`, `contextFilters`), whatever
wrote its row. A technical rule (telemetry and other datasets that give every reader the same rows, with no audience) is
evaluated and delivered to its contact points as before.

- **The audience in place of contact points.** A business rule is saved through `save#Definition` with
  `audience`: `recipientUserIds`, `recipientGroupIds`, `channels` (`notification`, the default, a private in-app
  notification on `topic`, `ReportAlert` without one; and `email`, the recipient's own account address through
  `emailTemplateId`), `recipientCap`, and the grouping and timing a contact point has (`groupBy`, `groupWaitSeconds`,
  `repeatSeconds`). Nobody named is the owner alone; the owner is told only when named or alone. A contact point, a shared
  contact reference, a webhook or any other audience field (an address, a URL) refuses the save
  (`BUSINESS_CONTACT_REFUSED`, `BUSINESS_AUDIENCE_REFUSED`). The server sets `audience.deliveryMode`: `personal` when the
  owner is the only recipient, `group` otherwise. A rule may also name `detailLink`, a plain path on this server its
  notifications link to with `?definitionId=` (an app's own rule screen), in place of Tools' Alerts.
- **At save.** An active business rule is saved only when every recipient passes the check as it stands for the rule
  (`RECIPIENT_REFUSED`, naming each with its reason code; a missing address refuses only where `email` is a channel) and
  the audience is within the cap (`RECIPIENT_CAP`, the cap and ceiling of report delivery); recipients other than the owner
  need `org.moqui.report.delivery.group`. `check#ReportAudience` answers the same for an alert rule the viewer reads. As
  for a schedule, a recipient must read the rule itself (`not-shared` otherwise: share it with a group they are in). A
  paused rule may be saved with a recipient to be mended.
- **Inactive until its owner activates it.** `copy#Definition` and `import#Definitions` of a business rule drop its contact
  points and save it paused (`pausedReason` `not-validated`), and so does a git sync's pull of a rule that is not already
  active here. Its owner activates it by saving it with `paused` false, which is the save above.
- **At each notification** every recipient is checked again and told separately, one notification or message a recipient;
  one whose scope changed is skipped with `scope-differs` (and the other codes of report delivery), one past the cap with
  `over-cap`, and one whose message fails is recorded and tried again at the next evaluation, while a recipient already told
  that state is not told it again until the repeat interval. Each recipient's last outcome for a notification group on a
  channel is a `moqui.report.AlertRecipientDelivery` row: the outcome, a safe reason code and, for one it was sent to, the
  state's name; never a value or a series label, and no state for one skipped. A business rule's notification row keeps no
  payload. The in-app notification goes to that one user: a subscription to every message of the topic
  (`NotificationTopicUser.allNotifications`) receives none of them (`NotificationMessage::direct_only`).
- **No Data, Error and stale are never Normal.** As for every rule, an empty result is `NoData` and a failed or incomplete
  query is `Error`; for a business rule so is a source whose index is older than its fields (the admission check `index`).
  A series removed after going missing (`pruneAfterMissing`) is told as Normal to a technical rule's contacts; a business
  rule's recipients are told nothing more of it (its last word is No data) and it leaves the rule's state.
  A business rule's state changes write no marker on the shared time series (`monitor_annotations`) and open no incident.
- **Reads.** `get#AlertState` answers a rule the viewer reads with each series' labels, state, times and last value
  (`AlertInstance.lastValue`): for a business rule to its owner, and to a recipient of its audience whose fingerprint equals
  the owner's now; anyone else gets `available` false with a reason code. Any other rule's state is answered to a reader of
  the rule who also reads its sources. It also answers what a screen needs to show any rule it reads: whether it is a
  business rule (`business`), a shipped template (`template`), paused, and its owner by name (`ownerName`; none for a
  shipped rule), so the [alert screens](#the-dashboards-app) read no alert instance and no account. `find#AlertDeliveries` lists the last outcome of each recipient of the rules the viewer
  owns, with `received` the deliveries to the viewer, and with `administrative`, for a holder of `ADMIN_TRANSFER_VIEW`, every
  owner's business rules as counters (sent, skipped, failed, paused, last evaluated) with no title, series, recipient or
  value.
- **Ownership.** A business rule whose owner cannot sign in pauses (`owner-disabled`) at its next evaluation and tells
  nobody. `find#OwnedAlertRules`, `pause#OwnedAlertRules` and `transfer#AlertRuleOwner` carry the owned-resource contract, as
  the schedule services do, for the user themselves or a holder of `REPORT_OWNER_CHANGE`. A transfer hands one rule, with the
  shared silences and mute timings it names that the leaving owner owns, to an active internal user who may save
  definitions, reads its sources and, for a group audience, may deliver to a group (`NEW_OWNER_REFUSED`); it stays paused
  (`transfer-pending`) until the new owner saves it active, which checks the audience against the new owner's fingerprint.
  A transfer removes the rule's series states and sent-notification records, which were the leaving owner's readings: the
  new owner's first evaluation starts from nothing.

The code is `crates/moqui-report/src/alert_delivery.rs` (with `alert.rs` for the evaluation); the regression spec is
`framework/test/BusinessAlerts.spec.js`.

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

## Changing settings for a while

Monitor › Settings changes a fixed set of logging and telemetry settings on a running server, without editing the
conf or restarting, for a while: to see full traces or DEBUG lines of one module while looking into a problem.

- **In memory only.** Each change is for 15 minutes, 1 hour, 4 hours or 24 hours, needs a reason, and expires on its
  own; Extend sets a new expiry, at most 24 hours from now. A restart forgets every change. Revert to on-disk ends one,
  Revert all every one.
- **On disk stays visible.** Each row shows its on-disk value and where it comes from: `conf` (the merged conf:
  MoquiDefaultConf, the components' MoquiConf, the runtime conf), `default` (the conf leaves it out), `env
  moqui_log_level`, or `as started` (the console). To keep a change, change it there and deploy (on production a git
  commit and the normal test and deploy).
- **Every running instance.** A change is published on the `runtime-settings` cluster topic and applied by every
  member; each expires it at the same absolute time. An instance that starts later has the on-disk values only.
- **What can change:**
  - Logging: the global level (up to DEBUG), the console level, and target rules: a module path (`moqui_core::service`)
    or dotted target (`moqui.monitor.audit`) and the targets below it at a level up to TRACE. rsipstack stays at WARN.
    DEBUG and TRACE lines go to the console and the log files, never to `otel_logs`, so they are not in Monitor › Logs.
  - Traces: `sample-ratio` and `slow-root-ms`.
  - Artifact types: `spans`, `metrics` and `sample-ratio` of each `telemetry/artifact` type.
  - Metrics: the `attributes` of each instrument.
  - Operations: `record` and `max-names`; a change resolves each service's operation again.
  - Browser: `enabled`, `sample-ratio`, `error-sample-ratio`, `replay-enabled` and `replay-sample-ratio` of
    `telemetry/browser`; a tab picks a change up on its next page load.
  - Receiver: `enabled`, `requests-per-minute` and `body-limit` of `telemetry/receiver`.
- **Not here:** `telemetry enabled` (the worker starts only when it is on at startup), the queue capacity and
  `concurrent-requests` (fixed at start), retention and the OTLP exporter.
- **Audit.** Each set, extend, revert and expiry is a WARN line on the `moqui.monitor.audit` target with the user, the
  key, the on-disk and new values, the expiry and the reason. The instance where it was made also writes a `config`
  annotation tagged `runtime-override` and the key: a set is a band to its expiry, an extend or an early revert a
  point. Incidents list them as changes, and dashboards draw them.

The same screen is System › Configuration › Logging and telemetry. Seeing it needs System access (`SYSTEM_APP`);
changing needs `MONITOR_SETTINGS`, which ADMIN holds and the public demo ends. The services are
`org.moqui.monitor.MonitorServices.list#RuntimeSettings`, `set#RuntimeSetting`, `revert#RuntimeSetting` and
`revert#AllRuntimeSettings`; the code is `crates/moqui-core/src/live_settings/` and
`crates/moqui-report/src/settings.rs`.

## Security

- Every query runs as the viewer. A dataset is authorized as the artifact `org.moqui.report.dataset.<id>`
  (`SYSTEM_APP` holds the `otel_*` ones; `REPORT_DATASETS_ADMIN` holds all), plus `DataDocumentUserGroup` for a
  DataDocument. A saved panel never widens access.
- A business dataset is read only once admitted, and its row filter is a registered scope policy
  ([Business datasets](#business-datasets)).
- A dataset that reads a telemetry index or a business dataset's document needs that source's own artifact, of its saver
  and of its reader or owner, whatever grants the dataset's own name ([Dataset aliases](#dataset-aliases)). So the update
  grant on `org.moqui.report.dataset` (saving dataset definitions) with a pattern over the saver's own dataset names is
  not a way to the logs or past a business dataset's row filter.
- Row filters (`rowFilter`, `contextFilters` from the signed-in identity, or a `rowFilterService`) are added by the
  server before documents, ranking, totals, aggregations and exports; a client never supplies them. Existing entity
  filters are not translated automatically: write the dataset filters an organization needs, and grant business roles
  only the datasets they should read.
- Alerts and scheduled reports run as their owner, whose access is checked at each run; recipients do not set what
  the report may read. A schedule over business data goes only to recipients whose scope equals the owner's, one message
  each ([Report delivery](#report-delivery)), and so does an alert over business data, which takes no contact point,
  address or webhook ([Business alerts](#business-alerts)).
- Queries have row, bucket and time limits, and a timeout. Exports and scheduled deliveries are logged to the
  `moqui.report.audit` target, so they show in Monitor Logs.
- Regressions and incidents need `MONITOR_INCIDENT`; approving, refusing, completing, cancelling and reverting
  recovery actions `MONITOR_RECOVER`; policies, adapters and the emergency stop `MONITOR_RECOVER_ADMIN`. ADMIN holds
  all three. Changing runtime settings needs `MONITOR_SETTINGS`. Editing eval datasets and scorers and scoring runs
  needs `MONITOR_EVAL`; starting and cancelling experiments, which start agent runs that cost money,
  `MONITOR_EVAL_RUN`. Their views and changes are logged on the `moqui.monitor.audit` target.

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
- **Monitor in System.** Monitor's routes are `/apps/system/Monitor/<Screen>` (`/apps/monitor` is gone, with no
  alias), Evals is `/apps/system/Llm/Evals`, and the `MONITOR_APP` artifact group is no longer shipped: its datasets,
  services and entities are members of `SYSTEM_APP`. On an existing database load
  `runtime/base-component/tools/data/ToolsSecurityData.xml` for those members. The database keeps its `MONITOR_APP`
  rows, which name a screen that is gone: they still grant the telemetry datasets and Monitor's services, and no
  longer any screen. A user group that was granted only `MONITOR_APP` needs `SYSTEM_APP` to open Monitor, which also
  opens the rest of System to it. The `MONITOR_*` permissions are unchanged. The old routes answer 404.
- **Business datasets** (Insights P3): the entity check creates `moqui.report.BusinessDataset`; load `ReportSetupData.xml` again
  for the `ReportScopePolicy` type and mr-mantle's `MonitoringSalesSecurityData.xml` for its policy. From then on a dataset with `signal`
  business is read only once admitted or by a holder of its update artifact (`ADMIN` holds them all), so a non-`ADMIN` reader of
  one sees it as not admitted until a steward admits it ([Business datasets](#business-datasets)).
- **Business events, drill and the semantic guards** (Insights P6): load `ReportSetupData.xml` (the `ReportAnnotationSource`
  type), `MonitoringSecurityData.xml` (`REPORT_BUSINESS_ADMIN`: the business annotation artifact and the restricted records
  artifacts, `ADMIN`'s) and mr-mantle's `MonitoringSalesSecurityData.xml` (its source) again. The annotations index gains four
  fields (`organizationPartyId`, `event`, `recordType`, `recordId`), added to an existing table in place. From then on a panel
  over a business dataset that sums an amount across several currencies without grouping by the currency says so
  (`UNIT_MIXED`) instead of showing a number without a unit: group it by its unit field or filter it to one.
- **Report delivery** (Insights P7): the entity check creates `moqui.report.ReportScheduleRecipient`, `ReportOccurrence` and
  `ReportRecipientDelivery` and adds `deliveryMode`, `recipientCap`, `pausedReason` and `versionNumber` to `ScreenScheduled`.
  Load `ReportSetupData.xml` (the occurrence and delivery states, `REPORT_OWNER_CHANGE`, the `ReportLegacyAdapter` type) and
  `MonitoringSecurityData.xml` (`REPORT_DELIVERY_ADMIN`) again. From then on an existing report schedule whose definition
  reads a business dataset sends each recipient their own message, and only to recipients whose scope equals its owner's;
  and a screen schedule that names a user group stops sending to the group until its screen path is registered as a
  `ReportLegacyAdapter` (its user's own delivery continues).
- **Business alerts** (Insights P8): the entity check creates `moqui.report.AlertRecipientDelivery` and adds `lastValue` to
  `AlertInstance`; no seed data changes. From then on a saved alert rule whose query reads a business dataset, or another
  whose rows depend on the viewer, tells its owner alone in the app until it is saved with an audience: the contact points
  its row holds (a notification to a group, an email address, a webhook) are no longer delivered to, a save that still
  names one is refused, and its state changes no longer write a marker on the time series or open an incident. Rules
  over telemetry are as they were.
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
