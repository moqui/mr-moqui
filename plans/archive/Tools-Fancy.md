# System and Tools homes

> Archived 2026-11-11: built, phases P1 to P6, then the gaps and most of §10 (F1 to F6). What was built and where it
> differs from the plan is in [§11](#11-as-built); the guides are the tools
> [README](../../runtime/base-component/tools/README.md), [Monitoring](../../docs/tools/Monitoring.md) and
> [Database](../../docs/tools/Database.md); what is still open is in
> [Remaining-Work](../Remaining-Work.md#system-and-tools-apps), and the deliberate differences from Java in
> [Parity-Diffs](../Parity-Diffs.md#system-and-tools-apps-2026-11-10).

Nov 10, 2026. Built, phases P1 to P6. Where the build differs from the plan below is in [§11](#11-as-built). The
decisions taken that day are in [§9](#9-decisions); none is open.

The tools component (`runtime/base-component/tools`) is the face of the framework: the first screens an administrator
or a developer opens on a new instance. This plan covers three things:

1. **App structure.** Monitor becomes a section of System, under System's own grant. That leaves Dashboards (any
   audience), System (the system administrator: administration and configuration) and Tools (the developer, the DBA
   and the technical administrator, on a dev instance or a server), with Dev AIDE beside them on a dev instance.
2. **Screen placement.** A review of which app each screen is in, the moves worth making, and the Monitor screens
   that should also show somewhere else.
3. **The two home screens.** `System/dashboard.xml` and `Tools/dashboard.xml` become `Home.xml`, redesigned for what
   their reader needs first, with the data and widgets that takes.

Utility comes first; the look follows from showing real, live information well. Nothing here restyles Quasar or adds
color outside the theme ([style guide](../../runtime/component/mr-boss/plan/style-guide.md) §1).

Contents: [1 today](#1-what-is-there-today) · [2 app structure](#2-app-structure) · [3 System home](#3-system-home) ·
[4 Tools home](#4-tools-home) · [5 building blocks](#5-building-blocks) · [6 modes, access, speed](#6-render-modes-access-and-speed) ·
[7 tests and docs](#7-tests-and-docs) · [8 phases](#8-phases) · [9 decisions](#9-decisions) · [10 later](#10-later) ·
[11 as built](#11-as-built)

## 1. What is there today

### 1.1 The apps

| App | Route | Menu index | Access | Menu |
| --- | --- | --- | --- | --- |
| Dashboards | `/apps/dashboards` | 96 | `DASHBOARDS_APP` | Dashboards, Explore, Library panels, Datasets, Alerts, Reports, Annotations, Git sync |
| Monitor | `/apps/monitor` | 97 | `MONITOR_APP` | 19 items in one flat list (Overview … Evals), none with an icon; 6 detail screens |
| System | `/apps/system` | 98 | `SYSTEM_APP` | 20 items in one flat list, plus Voice and Chat Bridges mounted by basic-comms |
| Tools | `/apps/tools` | 99 | `TOOLS_APP` | Data, SQL, Schema, ETL, Database, Service, Status Flows, Artifact Stats, Load testing |
| Dev AIDE | `/apps/dev` | 97 | dev instance only | Home, Workspace, Entities, Screens, Services, ECA, Artifacts, Data, Repositories, Tasks, Settings |

All four tools apps are granted to `ADMIN` (`data/ToolsSecurityData.xml`, `data/MonitoringSecurityData.xml`).

### 1.2 System › dashboard

Four blocks, top to bottom:

- **Instance overview**: lines of text in four columns (runtime, host, process, workers and search), from
  `ec.factory.getStatusMap(true)`; a toolbar with Open Monitor, Request details and Threads.
- **Navigation**: eight boxes of plain links in three columns. Every link has the same weight; Users sits beside
  Printers.
- **Database connections**: one row per entity group with the pool state as a sentence.
- **Loaded components**: a full-width table, the largest thing on the page. System › Components already lists the same
  rows with their dependencies (`LoadedList`).

What is wrong with it:

- It answers "what is this process" and not "is it healthy", "what is it doing" or "what needs me".
- Every number is one instant. There is no history, though `otel_metrics` holds a minute series for most of them.
- It knows nothing of the machine: no disk space, no system memory, no limit to compare the process's memory with.
- The listeners and integrations the server reports at startup (`moqui_host::startup_report`) are in the log only.
- The link boxes are hand-written paths. A screen another component mounts (Voice, Chat Bridges) is not there, and a
  screen a conf hides or replaces (the demo component does this) still is.
- It calls `ReportServices.list#Datasets` on every render to build one link.

### 1.3 Tools › dashboard

Three columns of link boxes (Data, ETL, SQL; Schema and database, Services and diagnostics, Load testing) and an API
reference box.

- The data grid, the most used tool, is a link named "Records" among five others of the same size.
- There is nothing about the instance: which database, how big the model is, whether this is production.
- Nothing is remembered: no recent entities, saved grid views, saved queries or runs, though the services for the last
  three exist (`find#GridViews`, `find#SavedQueries`, `find#SqlHistory`).
- There is no way from the page to one entity's records or one service: each goes through its tool's own picker.
- The API box builds the REST resource tree (`getFreshRootResources()`) and the master entity list on every render.
- The `reloadEcfi` transition has no link anywhere.

### 1.4 What a home can use as it is

| Thing | Where | Note |
| --- | --- | --- |
| Status map | `ec.factory.getStatusMap(true)` (`context/status.rs`) | The 1-minute load, processors, the process's memory used and peak, threads, start, uptime, host, data sources. Load, memory and threads are Linux only. No disk: the module says the standard library has no `statvfs` |
| Worker pool | `ec.factory.workerPool` | Active, pool size, maximum, queue, remaining capacity |
| Startup report | `moqui_core::startup`, rendered by `moqui_host::startup_report` | Listeners (HTTP, HTTPS, Cluster, gRPC, SSH, SMTP, Call media, TURN, SIP, XMPP, Edge tunnels) and integrations, each On with a detail, Off with the setting that keeps it off, Failed, Skipped or Not built; the tool factories; the routes. Not exposed to scripts |
| Metrics | `otel_metrics`, a document a minute per series, rolled up to hours and days | `http.server.request.duration` (route, method, status code), `moqui.artifact.duration`, `db.client.operation.duration`, `moqui.job.runs`, `moqui.operation.*`; gauges `process.memory.usage`, `process.thread.count`, `moqui.db.pool.checked_out`/`parked`/`waiting`/`idle`, `moqui.request.queue.depth`, `moqui.telemetry.dropped`/`failed` |
| `report-embed` | XSD; `WebrootReport.js`; TUI twin | A saved dashboard, one of its panels or a library panel on any screen, rendered as the viewer, with `from`/`thru` and variables; a compact header with the range and "Open in Dashboards". No time picker of its own |
| `report-panel` | same | One frame a screen computed, with a panel definition (Monitor › Metrics does this) |
| `dashboard`, `number-card` | XSD | A responsive grid of counters, each a link, with icon and delta |
| `link` with `icon`, `container-box` | XSD | Grouped link lists with icons need nothing new |
| `include-screen`, `transition-include` | XSD | One screen shown under a second parent: Monitor already wraps the Dashboards screens this way |
| Menu items | `sri.getMenuItems(path)` | Name, title, url, image, menu index and component of the subscreens the menu shows and the user may view |
| Alert rule templates | `report/*.json`, as mr-mantle's and mr-boss's | A shipped rule runs once an administrator copies it, with contact points |

### 1.5 Gaps

- The startup report cannot be read by a screen, and the build package, instance purpose and node role are not in
  the status map.
- No disk, system memory, memory limit, open-file or swap reading anywhere; no CPU series; the worker pool is not
  recorded.
- The data sources' pool numbers come as a sentence, not as numbers.
- There is no widget for a navigation tile (an icon, a title, a line of description, the whole card a link). The app
  launcher has the look, as raw markup.
- An embedded dashboard has no period control; the screen that embeds it supplies one.
- There is no single answer to "what needs attention": failed job runs, system messages in error, firing alerts, open
  incidents, new error groups and lost telemetry are each on their own screen.
- Tools remembers nothing about what the user opened.
- Monitor's screens have no menu icons.

## 2. App structure

### 2.1 Monitor into System

Monitor becomes the `Monitor` subscreen of System: `/apps/system/Monitor/<Screen>`, one entry in System's menu that
opens to its screens. Its screens keep their names and content.

- **Files.** `git mv screen/Monitor.xml screen/System/Monitor.xml` and `screen/Monitor/ screen/System/Monitor/`, so
  the file tree is the route, as the tools README says of System. `Monitor/dashboard.xml` becomes `Overview.xml`, its
  menu title and Monitor's default, in the same move (every Monitor route changes here once; the name `dashboard`
  goes from all three apps, §2.6). The `monitor` item leaves `MoquiConf.xml`. The location `component://tools/screen/Monitor` is named in ten
  files (the security data, `moqui-host/src/term.rs`, three specs, four documents, the conf).
- **Routes.** `apps/monitor` is written 161 times in 35 files: the tools screens and their `transition-include`s,
  three shipped dashboards (`report/monitor-*.json` data links), `WebrootReport.js` and its page test, two Rust
  sources (`moqui-report/src/recovery/execute.rs`, an incident link; `moqui-tui/src/components/report.rs`, the trace
  link), five Rust test files (`monitor_tui.rs`, `monitor_edge.rs`, `tools_screens.rs`, `openui_tests.rs`,
  `gap_tests.rs`), `acceptance/client/Monitoring.spec.js`, the agent test fixtures and the gallery's Record360. Each
  changes; no alias route stays ([practices](../../docs/dev/Practices-App-Dev.md) §3, one route per screen). The routes
  written in Rust and in `WebrootReport.js` are the place to give the Monitor path one constant each.
- **Access: `MONITOR_APP` goes.** Monitor is part of System and is granted with it.
  - The `MONITOR_APP` group and its `ADMIN` row leave `MonitoringSecurityData.xml`. Its members that are not the
    screen (the `otel_*` and `monitor_annotations` datasets, `MonitorServices`, the scrape and check services and
    entities, the agent eval services and entities) become members of `SYSTEM_APP` in `ToolsSecurityData.xml`. The
    screen needs no member: it is under `System.xml`, which is inherited.
  - So a holder of `SYSTEM_APP` reads the telemetry (traces with their users, logs, errors), in Monitor and, with
    `DASHBOARDS_APP`, in Explore.
  - The `MONITOR_*` permissions stay as they are: `MONITOR_INGEST`, `MONITOR_REPLAY`, `MONITOR_INCIDENT`,
    `MONITOR_RECOVER`, `MONITOR_RECOVER_ADMIN`, `MONITOR_SETTINGS`, `MONITOR_EVAL` and `MONITOR_EVAL_RUN` gate one
    sensitive thing each inside the section, not the section.
  - Monitor's nine wrappers of the Dashboards screens (Overview, Services, Operations, Logs, Metrics, Artifacts,
    Browser, Compare, Log context) stay, so a System administrator without `DASHBOARDS_APP` still follows a trace, a
    log line or a panel export.
  - What names the group: the demo lock-down (`DemoSecurityData.xml` and the list in `Demo.spec.js`; System's
    view-only row now covers Monitor), `MonitorSettings.spec.js` and `monitor_tui.rs` (their test groups are granted
    `SYSTEM_APP` instead; the rule they check, Monitor without `DASHBOARDS_APP`, is unchanged), a comment in
    `Monitoring.spec.js` and in `moqui-report/src/settings.rs`, `docs/tools/Monitoring.md` (four places) and
    `docs/security/Attack-Surface.md` (its table).
  - An existing database keeps its `MONITOR_APP` rows, which then name a screen that is gone. A group that was
    granted only `MONITOR_APP` needs `SYSTEM_APP`; Monitoring.md's Upgrading section says so.
- **Monitor › Overview stays.** The System home shows a strip of four charts and links to it; Overview is the full
  dashboard with its time picker and kiosk mode.
- **Menu order in Monitor**, by what is opened most: Overview, Services, Operations, Traces, Logs, Errors, Metrics,
  Artifacts, Browser, Replays, AI, Compare, Checks, Releases, Incidents, Recovery, Sources, Settings. Each gets a
  `menu-image`. Evals leaves for LLM and Agents (§2.3).

### 2.2 System's menu

Twenty-two entries in one list, in an order no one chose (Security and Service Jobs share index 12). There are no menu
groups in the drawer, so the home does the grouping ([§3.5](#35-navigation)) and the menu is put in the same order,
related entries together:

Home · Monitor · Components · Configuration · Caches · Service Jobs · Threads · Security · Audit Log · Visits ·
LLM and Agents · Constitution · System Messages · Entity Sync · Edge Node · Edge Clients · Voice · Chat Bridges ·
Localization · Git Repositories · Print · Hosted Instances

No route changes for this: only `menu-index` and titles.

### 2.3 Screen placement

Most screens are in the right app. The moves and renames, all decided ([§9](#9-decisions)), and what stays:

| Screen | Now | To | Why |
| --- | --- | --- | --- |
| Monitor (all) | Its own app | System › Monitor | Decided; §2.1 |
| Monitor › Evals | Monitor | System › LLM and Agents › Evals | Decided. Scores are database rows, not telemetry, and its users are the agent administrators. It moves in P1, so its routes and its terminal gate change once. `MONITOR_EVAL` and `MONITOR_EVAL_RUN` keep their names |
| The two `dashboard.xml` | `/apps/system/dashboard`, `/apps/tools/dashboard` | `Home.xml`, `/apps/system/Home`, `/apps/tools/Home` | Decided; §2.6 |
| System › Data Document | System | Tools › Data › Documents, after Views | Defining a DataDocument is modeling, beside Data › Views (`DbViewEntity`) and Data › Export (which exports documents). Feed indexing goes with it; the System home's Search row links to it. It moves in P1 (`git mv` of `System/DataDocument*` to `Tools/Data/Documents*`); four lines of `tools_screens.rs` and two of `plans/Monitoring.md` name the old route. It is then granted with Tools, not System |
| Tools › Artifact Stats | Tools | Stays; renamed "Artifact summary" | It counts entities, services and screens by package. The name reads as Monitor › Artifacts (hit performance). Its totals go on the Tools home as counters that link to it |
| Tools API reference (a box on the dashboard) | Tools › dashboard | Tools › REST API, its own screen | The home stops building the resource tree; the reference gets room for a filter |
| Tools › Database | Tools | Stays | DBA work. The System home shows the database's state (size, pool, backup) and links to it |
| System › Instances | System | Stays; renamed "Hosted Instances", last in the menu | It provisions application instances and their databases. "Instances" reads as the cluster's members, which have no screen (§3.4) |
| System › Resources | System | Stays; renamed "Git Repositories", icon `sym_o_folder_data` | Its only content is the git repository list; the icon is a magnifier |
| System › Thread List | System | Stays; "Threads" | Live process state; linked from the home's worker counter |
| System › Constitution | System | Stays, next to LLM and Agents in the menu | Moving it under LLM changes routes that `moqui-llm` writes, for no gain |
| Tools › Data › Load, Export, Snapshots | Tools | Stay | Data work, already gated; the demo conf hides them by path |
| Terminal UI, Site Map | webroot (`/apps/Terminal`, `/apps/ScreenTree`) | Stay; linked from the Tools home | Useful to the Tools reader and not in any menu |
| Dev AIDE | dev component | Stays; the Tools home links to it when it is loaded | Tools reads and runs; Dev AIDE edits. The entity and service screens of each link to the other's on a dev instance |
| `Tools/dashboard.xml` `reloadEcfi` | Tools | Remove | No link reaches it |

### 2.4 Labels

"Conf" → "Configuration"; "Sys-Sys Messages" → "System Messages"; "Cache" → "Caches"; "Thread List" → "Threads";
"Instances" → "Hosted Instances"; "Resources" → "Git Repositories"; "Artifact Stats" → "Artifact summary";
"Data Document" → "Documents". Titles only; the routes of these screens do not change, except Documents', which moves.

### 2.5 Monitor in other places

Monitor stays one section, and some of what it has also belongs where the reader already is. Three ways to show it
there, none of which copies logic:

- **A wrapper screen**: a few lines that `include-screen` the Monitor screen and take in its transitions with
  `transition-include`, as `Monitor/Split.xml` (Compare) wraps the Dashboards screen today. The Monitor screen stays
  the owner; the wrapper adds a place in a second menu. This is a second mount of a shared screen, not an alias of a
  moved one.
- **`report-embed`** of a shipped dashboard, one panel of it, or a library panel, with variables that narrow it to the
  record on the page.
- **A count and a link** on the System home (§3.4).

| From Monitor | Also in | How |
| --- | --- | --- |
| Settings: log levels and telemetry settings changed for a while | System › Configuration, a "Logging and telemetry" tab beside Settings, Edit and Changes | Wrapper `System/Conf/Logging.xml` over `Monitor/Settings.xml`, with `setSetting`, `revertSetting` and `revertAll`. Changing still needs `MONITOR_SETTINGS` |
| AI: traces with model calls | System › LLM and Agents › Traces, beside Calls, Runs and Evals | Wrapper over `Monitor/AgentTraces.xml`, with `traceDetail`, `dashboard` and `usage` |
| The AI dashboard (`monitor-ai`) | LLM and Agents › Overview, above today's totals | `report-embed` of its `calls` and `tokens` panels over seven days |
| Artifacts (`monitor-artifacts`) | Tools › Service, a service's detail; Tools › Schema, an entity, when entity metrics are on | `report-embed definition-id="monitor-artifacts" panel-id="activity"` with `variables="{artifactType: 'AT_SERVICE', artifactName: serviceName}"`: this one artifact's hits and duration over seven days, and a link to Monitor › Artifacts narrowed to it |
| Artifacts, as a list | Tools home, "Artifact performance" under Services and reference | A link to Monitor › Artifacts for services |
| Database statements and the pool | Tools › Database › Overview | `report-embed` of a shipped library panel: connections in use and waiting over the last hour |
| Checks: synthetic checks and scrape targets | System home, Needs attention | "N checks failing", "N scrape targets down", from each row's last result |
| Sources: where telemetry is kept and sent, and what was lost | System home | The Telemetry row of Integrations; "Telemetry lost" in Needs attention |
| Incidents, Errors | System home, Needs attention | Counts with links |
| Traces of one user | System › Security, a user's detail | A link "Requests by this user" into Monitor › Traces with the user filter. A visit already lists its requests from the spans, and a job run links to its trace and logs |

An embed does not hide itself from a reader who may not read its dataset: each of its panels says "Not available:
viewer may not read dataset …". So the three embeds in Tools (a service's detail, an entity's, Database › Overview) are
guarded in the screen by a check that the reader may read `otel_metrics`, and a Tools user without System sees those
pages as they are today.

### 2.6 Home, not dashboard

`System/dashboard.xml` and `Tools/dashboard.xml` become `System/Home.xml` and `Tools/Home.xml` (`git mv`), titled
Home, with `default-item="Home"`: `/apps/system/Home` and `/apps/tools/Home`. The old routes are written on about
fifty lines in nineteen files outside the plans, most of them tests that use the System page as "a page a signed-in
administrator can open":

- the agent test `tools/test/agent/assist.agent.js` and one recorded replay (16 lines; the replay is a recording, so
  it gets the same scripted replacement and its spec is run);
- the framework's security client specs, `SecurityScreens`, `SecurityAccess` and `SecurityTarpit` (12);
- `LoadBasic.spec.js` (4), `WebrootRum.page.js` (2), a test fixture's `Http.spec.js` (1), `AdminOverview.spec.js` (1);
- three Rust host tests, `agent_eval.rs`, `tools_services.rs` and `test_client.rs` (4);
- `Tools/Database/SpeedTest.xml` (a link), the two app roots;
- four guides: `Proof-Tests.md`, `Attack-Surface.md`, `Client-Testing.md`, `Build-Test.md`.

Monitor's `dashboard` is on fifteen more lines in four test files; it becomes `Overview` with the move (§2.1).

## 3. System home

### 3.1 What it answers, in order

1. Is this instance healthy now?
2. What is it: version, build, purpose, role, host, how long it has been up?
3. What has it been doing: traffic and resources over the last hour, or a longer period?
4. What is switched on: which ports, interfaces, integrations and tools, and did anything fail to start?
5. What needs me?
6. Where do I go from here?

### 3.2 Layout

Desktop, 1920 wide with the drawer open; it works from 1024.

```
┌──────────────────────────────────────────────────────────────────────────────────────────────────────────┐
│ MR Moqui 0.1.0 · maximal · production · role all          [Ready]  [3 need attention]                    │
│ erp1.example.com · linux 6.8 · x86_64 · 16 processors · up 3 d 4 h (since Nov 7, 02:14) · 14 components  │
│                                               Period [1h] 6h 24h 7d    Updated 14:02:31  [Refresh] [More]│
├──────────────┬──────────────┬──────────────┬──────────────┬──────────────┬───────────────────────────────┤
│ LOAD         │ MEMORY       │ DISK         │ WORKERS      │ DB POOL      │ USERS TODAY                   │
│ 31 %         │ 512 MiB      │ 91 %         │ 3 of 22      │ 4 of 90      │ 12                            │
│ 5 minutes    │ 3 % of 16 GiB│ /var         │ 0 queued     │ 0 waiting    │                               │
├──────────────┴──────────────┴──────────────┴──────────────┴──────────────┴───────────────────────────────┤
│ Activity · last hour                                                             Open in Dashboards      │
│ ┌ Requests and errors ───┐ ┌ Response time p50 p95 ─┐ ┌ CPU load ──────────────┐ ┌ Memory ──────────────┐│
│ │  ▂▃▅▆▅▃▂▃▅▇▆▅          │ │  ▁▁▂▁▁▃▁▁▂▁▁▁          │ │  ▂▂▃▃▂▂▅▆▃▂▂▂          │ │  ▅▅▅▅▆▆▆▆▆▆▆▆        ││
│ └────────────────────────┘ └────────────────────────┘ └────────────────────────┘ └──────────────────────┘│
├───────────────────────────────────────────────┬──────────────────────────────────────────────────────────┤
│ Needs attention                               │ Listeners                                                │
│ ▲ Disk /var is 91 % full, 4 GiB free          │  HTTP        tcp 0.0.0.0:8080                 on         │
│ ▲ 3 job runs failed in 24 hours     Job runs  │  HTTPS       tcp 0.0.0.0:8443                 on         │
│ ▲ The database backup job is paused   Backup  │  gRPC        tcp 0.0.0.0:4317                 on         │
│ ● 2 system messages in error        Messages  │  SSH         ssh_port is not set              off        │
│                                               │ Integrations                                             │
│ Resources                                     │  Search      postgres                         on         │
│  CPU load    31 % of 16 processors            │  Telemetry   kept in the search store         on         │
│  Memory      512 MiB of 16 GiB, 9 GiB free    │  Scheduler   checks every 60 s                on         │
│  /var        91 % of 50 GiB, 4 GiB free       │  PDF         no Chromium found                off        │
│  /           38 % of 100 GiB                  │  LLM         default model …                  on         │
│  Open files  412 of 65,536                    │  MCP server  mcp_enabled is not true          off        │
│                                               │                                                          │
│ Database                                      │ Tools: …                                                 │
│  transactional  embedded PostgreSQL 18        │ Not in this build: …                                     │
│  2.1 GiB · pool 4 of 90 · backup daily, on    │                                         [Routes]         │
├───────────────────────────────────────────────┴──────────────────────────────────────────────────────────┤
│ ┌ Monitor ─────────┐ ┌ Users and access ┐ ┌ Service jobs ────┐ ┌ Configuration ───┐ ┌ Components ───────┐│
│ │ icon  traces,    │ │ icon  accounts,  │ │ icon  schedules, │ │ icon  settings   │ │ icon  loaded,     ││
│ │ logs, errors …   │ │ groups, access   │ │ runs, results    │ │ and their source │ │ deploy, update    ││
│ └──────────────────┘ └──────────────────┘ └──────────────────┘ └──────────────────┘ └───────────────────┘│
│ Server           Monitor          Security and history   Integration        AI              Content      │
│  Caches           Services         Users                  System messages    Models          Localization│
│  Threads          Traces           User groups            Remote systems     Runs            Git repos   │
│  Job history      Logs             Artifact groups        Entity sync        Evals           …           │
│  …                …                …                      …                  …                           │
└──────────────────────────────────────────────────────────────────────────────────────────────────────────┘
```

At 1024 the counters wrap to three a row, the charts to two, and the two middle columns stack. On a phone everything
is one column, counters two a row, and the link groups collapse to their titles.

### 3.3 The blocks

| Block | Shows | From | Goes to |
| --- | --- | --- | --- |
| Identity | Version, build package, instance purpose, node role; host, OS, architecture, processors; uptime and start; component count | The runtime report (B1), the status map | Components (the count) |
| State | Ready, or Draining, or Not ready, as a word with its color; the count of attention items | Readiness as `/health` has it (B2); §3.4 | The attention block |
| Period | 1h (default), 6h, 24h, 7d, as links carrying `period` | A screen parameter | — |
| More | Request details (the dialog of today), Threads, the `/metrics` and `/health` addresses and who may read them | `ec.web.getRequestDetails()`, the webapp's `status-ips` | — |
| Counters | Load (the 5-minute load as a share of the processors); memory (the process's, and its share of the limit); disk (the fullest filesystem's share used, with its path); workers active of maximum, with the queue; database connections checked out of the pool, with those waiting; users today (those with a visit since midnight). A counter at a warning or critical level takes that color | The resource readings (B2), `ec.factory.workerPool`, a count of `moqui.server.VisitUserSummary` rows for today | Monitor › Metrics on that metric; the Resources block; Threads; Tools › Database › Activity; Active Users for today |
| Activity | Four time series over the period: requests and 5xx responses per minute; response time p50 and p95; CPU load (the system's and this process's); memory | `report-embed` of the shipped `system-home` dashboard (B4), over `otel_metrics` with the new gauges (B3) | Open in Dashboards; Monitor › Overview |
| Needs attention | §3.4 | `get#SystemAttention` (B5) | Each item's own screen, filtered |
| Resources | CPU load over 1, 5 and 15 minutes and this process's share; memory: the process, the system's total and available, the limit in force (a container's, else the system's); each filesystem the instance writes to, with its share used, its free space and what is on it (runtime, database, logs, temp); open files of the limit; swap when there is any. Each row has its level as a word | The resource readings (B2) | Monitor › Metrics on that reading's series |
| Database | Each entity group's database: kind and version, size, pool, and for the embedded database its backup job's state | The status map; `DatabaseServices.get#Capabilities` (kept five minutes) | Tools › Database › Overview |
| Listeners and integrations | Every listener with its address or why it is off; every integration with its detail; failed and skipped ones first, in the negative and warning colors; the tool factories; what the build leaves out; the routes in a dialog | The runtime report (B1) | The screen that configures each, where there is one: Search → Tools › Data › Documents, Telemetry → Monitor › Sources, LLM, MCP, A2A and the gateway → LLM and Agents, Edge link → Edge Node, Scheduler → Service Jobs, PDF and IPP → Print, Call media, SIP and XMPP → Voice and Chat Bridges, the rest → Configuration |
| Navigation | §3.5 | The navigation catalog (B7) | — |

A reading the platform does not report shows "—" with a tooltip, as today's "Unavailable"; it is never zero, and it
raises no attention item.

### 3.4 Needs attention

One list, most severe first; each row is a word and a color (never a color alone), what is wrong with its number, and
a link. With nothing to show it says "Nothing needs attention" and when it was checked.

**Resources.** Each reading has a level, worked out once in Rust from the conf's thresholds (B2), so the counter, the
Resources row, the attention item and an alert rule agree. The first defaults, as shares used:

| Reading | Warning | Critical | Says |
| --- | --- | --- | --- |
| Disk space, each filesystem | 80 % | 90 %, or under 1 GiB free | "Disk /var is 91 % full, 4 GiB free" and what is on it |
| Disk inodes, each filesystem | 80 % | 90 % | "Disk /var has used 91 % of its file entries" |
| Memory, the process against the limit in force | 80 % | 90 % | "The server uses 82 % of the 4 GiB it may use" |
| Memory, the system's available | 10 % left | 5 % left | "The machine has 6 % of its memory available" |
| CPU load, the 5-minute average over the processors | 80 % | 100 % | "CPU load is 96 % of 16 processors over five minutes" |
| Open files against the limit | 80 % | 95 % | "412 of 512 open files" |
| Swap in use | 50 % | — | "Swap is 61 % used" |

The 5-minute load is used so a burst does not raise and clear an item between two page loads; disk and memory move
slowly and are read as they are.

**Saturation and state.**

| Item | Shown when | Link |
| --- | --- | --- |
| A listener or integration failed or was skipped | The runtime report has one | Configuration |
| The instance is draining | The drain flag is set | Monitor › Recovery |
| Workers or the database pool are saturated | No queue capacity left, every worker active, or connections waiting | Threads; Tools › Database › Activity |
| Cluster member silent | A `ClusterMember` heartbeat older than the limit, when clustered | The members list (below) |

**Work that failed or waits.**

| Item | Shown when | Link |
| --- | --- | --- |
| Job runs failed | Any in the last 24 hours | Job Runs, failed, that range |
| The embedded database has no running backup | The backup job is missing or paused | The job |
| System messages in error | Any | System Messages, that status |
| Open incidents | Any, for a holder of `MONITOR_INCIDENT` | Monitor › Incidents |
| Alerts firing | Any rule the user may read | Dashboards › Alerts |
| Checks failing, scrape targets down | A check's or target's last result failed | Monitor › Checks |
| New error groups | First seen in the last 24 hours | Monitor › Errors |
| Telemetry lost | Dropped or failed counters above zero since the start | Monitor › Sources |
| Agent approvals waiting | Any, for an agent administrator | LLM and Agents › Approvals |

When the instance is clustered (the Cluster listener is on), a small **Cluster members** list sits under Database:
instance id, base URL and heartbeat age. There is no screen for it today.

The attention list is seen only by someone looking. For the resources the plan also ships alert rule templates (B4)
over the same gauges, so a copied rule sends its notification, email or webhook.

### 3.5 Navigation

Two tiers, both built from the catalog (B7), so a hidden or replaced screen has no entry and a mounted one does.

- **Featured** (`nav-card`, B6): Monitor, Users and access, Service jobs, Configuration, Components. An icon, a title
  and one line saying what is there.
- **Groups**: a `container-box` per group of icon links. Server (Caches, Threads, Job history, Printers, Print jobs,
  Hosted instances) · Monitor (Services, Traces, Logs, Errors, Metrics, Incidents, Checks, Settings, then "All of
  Monitor") · Security and history (Users, User groups, Artifact groups, Active users, Audit log, Visits) ·
  Integration (System messages, Remote systems, Message types, Entity sync, Edge node, Edge clients, and Voice and
  Chat bridges when basic-comms mounts them) · AI (Overview, Models, Tools, Runs, Approvals, Usage, Traces, Evals,
  Constitution) · Content (Localized messages, Localized entity fields, Git repositories).

### 3.6 What leaves the page

- The loaded components table: Components has it. The home keeps the count.
- The database connections table: the Database block, with numbers.
- The eight link boxes: the catalog's groups.
- The `list#Datasets` call: "Explore documents" becomes a plain link to Dashboards › Explore, which picks its own
  default dataset.

### 3.7 States

- **Telemetry off or not built.** The Activity strip is replaced by one line: "Telemetry is off, so there is no
  history" with the setting and a link to Configuration. The counters, the Resources block and the resource attention
  items read the machine, not the telemetry, and still show.
- **A new instance.** Charts with a few points and "No data in this time range" are the report widgets' own states.
- **Off Linux.** macOS reports the filesystems and nothing else of the machine; Windows reports none of it for now
  (B2). What is not reported is "—".
- **In a container.** The memory limit is the container's, and the filesystems are the ones mounted into it.
- **An external database.** Its disk is on another machine and is not on this page; Tools › Database has its size.
- **A limited user.** Each block's service checks its own permission and leaves out what the user may not read; a
  block with nothing left is not rendered.
- **A cluster.** Counters and resources are this process's and this machine's. The embed passes this instance's id as
  the dashboard's `instance` variable so the charts agree with them; Open in Dashboards offers All.

## 4. Tools home

### 4.1 What it answers

1. Which instance and database am I about to work on, and what am I allowed to do here?
2. Take me to this entity, or this service, now.
3. Where are the main tools?
4. What was I doing, and what is running?
5. Where is everything else?

### 4.2 Layout

```
┌──────────────────────────────────────────────────────────────────────────────────────────────────────────┐
│ production · embedded PostgreSQL 18 · database: transactional [change]      You can: SQL · Database admin│
│ This is a production instance. Changes made here are changes to live data.                               │
├──────────────────────────────────────────────────┬───────────────────────────────────────────────────────┤
│ Open an entity  [ search by name …            ▾] │ Open a service  [ search by name …                 ▾] │
├────────────┬────────────┬────────────┬───────────┴┬────────────┬─────────────────────────────────────────┤
│ ENTITIES   │ VIEW ENT.  │ SERVICES   │ SCREENS    │ ECA RULES  │ DATABASE SIZE                           │
│ 1,412      │ 386        │ 2,380      │ 1,105      │ 214        │ 2.1 GiB                                 │
├────────────┴────────────┴────────────┴────────────┴────────────┴─────────────────────────────────────────┤
│ ┌ Data grid ─────────────────────┐ ┌ SQL ────────────────────────────┐ ┌ Entities ─────────────────────┐ │
│ │ icon  Find and edit records of │ │ icon  Run statements, read only │ │ icon  Definitions, tables and │ │
│ │ an entity, a view, a SQL result│ │ or with changes confirmed       │ │ their differences             │ │
│ │ or a file                      │ │                                 │ │                               │ │
│ │ Open a file · Saved views      │ │ History · Saved queries         │ │ Check tables · Indexes, keys  │ │
│ └────────────────────────────────┘ └─────────────────────────────────┘ └───────────────────────────────┘ │
│ ┌ Services ──────────────────────┐ ┌ Database ───────────────────────┐ ┌ ETL ──────────────────────────┐ │
│ │ icon  Inputs, results and the  │ │ icon  Sessions, statements,     │ │ icon  Mappings that load a    │ │
│ │ rules of any service; run one  │ │ tables, maintenance and jobs    │ │ file or a query into entities │ │
│ │ Run a service                  │ │ Activity · Tables · Jobs        │ │ Runs · Value maps             │ │
│ └────────────────────────────────┘ └─────────────────────────────────┘ └───────────────────────────────┘ │
├──────────────────────────────────────────────────┬───────────────────────────────────────────────────────┤
│ Pick up where you left off                       │ Running and recent                                    │
│  mantle.order.OrderHeader            entity      │  ETL  Price list import       Completed   14:01       │
│  Open orders by customer             grid view   │  Job  Reindex OrderItem       Running     13:58       │
│  Stuck shipments                     saved query │  Load order-to-cash           Completed   yesterday   │
├──────────────────────────────────────────────────┴───────────────────────────────────────────────────────┤
│ Data             Schema and database       Services and reference     Load testing      Elsewhere        │
│  Browse related   Indexes and keys          Status flows               Load scripts      Terminal UI     │
│  Views            Statement statistics      Artifact summary           Runs              Site map        │
│  Documents        Database settings         REST API                                     Dev AIDE        │
│  Load             Speed test                Entity master API                            System › Monitor│
│  Export                                     Artifact performance                                         │
│  Snapshots                                                                                               │
└──────────────────────────────────────────────────────────────────────────────────────────────────────────┘
```

### 4.3 The blocks

| Block | Shows | From | Notes |
| --- | --- | --- | --- |
| Instance | Purpose, database kind and version, the selected database with the picker; what the user holds of `SQL_RUNNER_WEB`, `DATABASE_ADMIN`, `DATABASE_EXTERNAL`, `LOAD_RUNNER`, the missing ones muted with a tooltip naming the permission | The runtime report (B1), `template/DatabaseParts.xml`, `ec.user.hasPermission` | On a production instance one `q-banner` line says so. This is why a tile is absent, said once |
| Jump | "Open an entity" goes to Data › Grid on it (the row's second action: its definition); "Open a service" goes to its detail with Run | Two server-search drop-downs over the entity names and the known service names, with `submit-on-select` | The single most useful thing on the page for a developer. Works in the terminal as drop-downs |
| Counters | Entities, view entities, services, screens, ECA rules; the selected database's size | The counts Artifact summary computes; `get#Capabilities` (kept five minutes) | Each links to its list. If a count proves slow it is cached with the model, not dropped |
| Featured | Six `nav-card`s, `size="lg"`: Data grid, SQL, Entities, Services, Database, ETL, each with up to three secondary links | The catalog (B7) | SQL only with `SQL_RUNNER_WEB`; five tiles then fill the row |
| Left off | The entities the user opened last in the grid (new: B8), saved grid views, saved queries and the latest SQL history rows | The `tools.recent` preference, `find#GridViews`, `find#SavedQueries`, `find#SqlHistory` | Ten rows; each opens the thing. Hidden when empty |
| Running and recent | The latest ETL runs, database jobs and load runs, with a state chip | The Etl, Database Jobs and Load services | Five rows each, by start; Load only with `LOAD_RUNNER`. Hidden when empty |
| More | Grouped icon links: everything not featured | The catalog | Terminal UI, Site map, Dev AIDE (only when its app is mounted) and System › Monitor under Elsewhere; Artifact performance goes to Monitor › Artifacts (§2.5) |

### 4.4 What leaves the page

- The API reference box: Tools › REST API ([§2.3](#23-screen-placement)), linked from Services and reference.
- The External databases link and its `canExternal` probe (a service call on every render to decide one link).
  Connections is not a menu screen; Tools › Database › Overview already makes the same probe and links to it.
- `reloadEcfi`.

## 5. Building blocks

| # | Block | Kind | What |
| --- | --- | --- | --- |
| B1 | Runtime report for scripts | Rust, `moqui-core` script host | `ec.factory.getStartupReport()`: `{version, package, purpose, role, readySeconds, listeners: [{name, state, detail}], integrations: [...], tools: [...], notBuilt: [...], routes}`, the same entries `startup_report::log` renders, read from the factory's report. The host records the routes and the ready time when it logs. State is one of `on`, `off`, `failed`, `skipped` |
| B2 | Resource readings | Rust, `context/status.rs`; conf | A `Resources` map in the status map, each reading with its level (`ok`, `warning`, `critical`, or null when not reported). **CPU**: the 1, 5 and 15-minute load (`/proc/loadavg`), the 5-minute share of the processors, and this process's CPU share between two reads (`/proc/self/stat`; `moqui-host`'s load runner has this reader, which moves here so there is one). **Memory**: the process's resident and peak, the system's total and available (`/proc/meminfo`), the limit in force (cgroup `memory.max` when set, else the total), swap. **Filesystems**: one row per filesystem under the runtime directory and those of its directories on a filesystem of their own (`db`, `log`, `tmp`, `txlog`, `opensearch`), and the system temp directory: total, available to the server's user, share used, inodes used, what is on it. From `rustix::fs::statvfs`: `rustix` is already a dependency of `moqui-core`, so this adds its `fs` feature, no crate and no `unsafe`. **Open files**: the count of `/proc/self/fd` and the soft limit of `/proc/self/limits`. The readings are kept for ten seconds, so a page, the gauges and the attention service share one read, and no thread wakes for them. Also in the status map: each data source's pool as numbers beside `detail`; `Ready` and `Draining`; the request pool. **Thresholds**: a `<resource-watch>` conf element with a warning and a critical value for each reading, the defaults of §3.4. **Platforms**: Linux has all of it; macOS the filesystems; Windows none until a reader without `unsafe` is chosen (§10) |
| B3 | Gauges | Rust, `telemetry::register_runtime_gauges` | From B2, once a minute: system load (1 and 5 minutes, and the share of the processors), process CPU, memory (the system's available, the share of the limit; the process's is recorded today), each filesystem's share used and free bytes (one series per filesystem, its mount point allowed as the metric's attribute in the default conf), open files, service workers active and queued, request workers busy. Named after the OpenTelemetry host and process metrics where one exists. Ten or so more documents a minute in `otel_metrics`, plus one per filesystem, kept by the existing retention; on a quiet instance this is the plan's whole storage cost, and it follows `telemetry_enabled` |
| B4 | Shipped definitions | `report/*.json`, folder System | `system-home`: four time series panels in one row, an `instance` variable. A library panel of the database pool (§2.5). Three alert rule templates over the gauges: low disk space, high memory use, high CPU load, each with the thresholds of §3.4; a template runs once copied, which gives it an owner and contact points |
| B5 | `get#SystemAttention` | Service, `service/moqui/tools/SystemHomeServices.xml` | Returns the items of §3.4 as `{severity, text, count, link, parameters}`: the resource readings whose level is not `ok`, then the others. Each item is checked on its own: a failure logs an error naming it and the rest still come back ([AGENTS.md](../../AGENTS.md), failure tolerance). Each checks the permission of what it reads and is left out without it. Counts only, each with a limit |
| B6 | `nav-card` | XML screen element | `<nav-card title text icon link size="md|lg" condition style>` with up to three child `link`s. qvt: `m-nav-card`, a flat bordered `q-card` that is one link, the icon in a tonal avatar, the title `text-subtitle1 text-weight-medium`, the text `text-caption m-muted`, the child links as text links along the bottom. html: a card with an anchor. tui: a link node with the title and the text. text and print: "Title: text". Laid out by `dashboard`. The whole [checklist](../../runtime/component/mr-boss/plan/style-guide.md#81-adding-an-xml-element-the-checklist) applies: XSD, each render mode's macro file and the qvt static mirror, the wrapper and its precompiled render, the CSS class, the goldens, a `Qvt-Client.md` row and a gallery screen |
| B7 | Navigation catalog | Screen actions and a shared section, `template/HomeParts.xml` | Each home lists its entries once: `{group, path, text, featured, links}`. An entry names a screen that is in a menu; a screen out of the menu (a detail screen, Connections) is reached from its parent. The shared section drops an entry the menu does not show for this user (`sri.getMenuItems` of its parent, which leaves out what `menu-include` or a conf override hides and what the user may not view), takes the title and icon from the menu item, and puts menu items no entry names in a last group, so a screen a component mounts is never missing. A component adds or changes entries with a `screen-extend` `actions-extend`, as `AppList.xml`'s suites |
| B8 | Recent entities | Screen, `Tools/Data/Grid.xml` | Opening an entity in the grid puts its name first in the user preference `tools.recent` (twenty names), as `REPORT_RECENT` does for dashboards |

`nav-card` is an element, not raw markup, because it has two users on the day it is built (the two homes) and likely
more after: Dev AIDE's Home, and the shortcuts block of every BOSS landing (style guide §3.1, and §4.8 "a second use
is a widget").

## 6. Render modes, access and speed

- **qvt and tui.** Both homes are plain XML screens over existing widgets plus `nav-card`, so the terminal gets them
  from the macros: counters as lines, the embedded dashboard as its TUI twin, lists as lists, tiles as links. Nothing
  needs a `.qvue`. A level is a chip in the browser and a word in the terminal, each from its own `render-mode` text.
- **html** (Standard, `/apps`): the plain fallbacks of the same widgets.
- **Server-static.** A screen with `report-embed` is not a server-static template; the System home is live data and
  does not want to be. The Tools home has no embed and stays static-capable: its permission flags and counts are set
  in actions, as today.
- **No writes.** The period is a URL parameter and not a saved preference, so the home works for a view-only
  administrator (the demo lock-down). The only write in this plan is B8, in the grid, which a view-only user's
  authorization already decides.
- **Access.** Each block's data comes from a service that checks the reader, not from the screen hiding it.
- **Speed.** The status map, the worker pool and the runtime report are in-process reads; the resource readings are a
  few small files and one `statvfs` per filesystem, at most every ten seconds. The attention service is a handful of
  bounded counts. The embed is four queries over minute documents. Time the home against Monitor › Overview on a
  loaded demo database; if the embed is most of it, load the Activity strip in a `dynamic-container` after the page
  paints, so the rest never waits on the search store. The Tools home drops two calls it makes today (the REST tree,
  the external-database probe) and adds two short lists.
- **Devices.** Both pages are checked at 1920 and at 390, light and dark, and in `moqui tui`.

## 7. Tests and docs

Framework work: `./moq test-framework` before each commit.

- **Regression** (`tools/test/`), one html render of each home with all of its checks, replacing the dashboard check
  of `AdminOverview.spec.js`: no `Error rendering`, a text present whatever the data, a listener row, the empty
  attention state. Rule tests, not renders: `get#SystemAttention` (a failed job run is counted; a resource at warning
  is listed with its number; an item whose source the user may not read is left out; one item's failure leaves the
  others), the catalog (an entry whose screen a conf hides gives no tile; a mounted screen with no entry is in the
  last group).
- **Rust**: the runtime report as a script value (states, order, a second record replacing the first); the levels
  from the thresholds (below, at and above each, and the free-space floor); the readings parsed from sample `/proc`
  texts, a container limit over the system's, a reading that is not there as null and not zero; one filesystem
  listed once when two directories are on it; the gauges registered once.
- **`nav-card`**: the qvt and tui render goldens and a node test of the client, as the checklist has it. No screen
  sweep.
- **Changed, not new**: `monitor_tui.rs`, `acceptance/client/Monitoring.spec.js`, `MonitorSettings.spec.js`,
  `Demo.spec.js`, the security client specs and the agent fixtures take the new routes, and those that granted
  `MONITOR_APP` grant `SYSTEM_APP`. The acceptance ones are run because they changed, once.
- **Docs**: the tools `README.md` (three apps; Monitor under System; what each home's block reads),
  `docs/tools/Monitoring.md` ("The Monitor app" becomes a section of System without a grant of its own; "Monitor › X"
  stays the short name of its screens; a line under Upgrading), `docs/security/Attack-Surface.md` (the `MONITOR_APP`
  row folds into `SYSTEM_APP`; the routes), the four guides that name the old home routes, `docs/Run-Deploy.md` (the
  `resource-watch` thresholds), `docs/ui/Qvt-Client.md` and the gallery for `nav-card`. `plans/UI-Refinement.md`'s
  inventory is a record of its time and stays.

## 8. Phases

Each is its own commit, usable on its own. Every route and grant changes in P1 and nowhere else.

| Phase | What | Usable when |
| --- | --- | --- |
| P1 | Routes and access: Monitor into System with `MONITOR_APP` folded into `SYSTEM_APP` (§2.1), Evals to LLM and Agents, Data Document to Tools › Data › Documents, `dashboard` to `Home` and `Overview` (§2.6), the menu order, icons and labels (§2.2, §2.4). The home screens keep today's content | `/apps/system/Monitor/...`, `/apps/system/Llm/Evals`, `/apps/tools/Data/Documents` and the two `Home` routes work in the browser and the terminal; `/apps/monitor`, `/apps/system/DataDocument` and the `dashboard` routes answer 404; an `ADMIN` without a `MONITOR_APP` row uses all of Monitor; the tests pass on the new routes |
| P2 | Data: B1, B2, B3, B4, B5 | A script reads the runtime report and the resource readings; the new gauges chart in Monitor › Metrics; `system-home` opens in Dashboards; a copied disk alert fires on a full test filesystem; the attention service answers |
| P3 | `nav-card` (B6) with its gallery screen | The gallery shows it in both sizes, light and dark, and in the terminal |
| P4 | System home (§3), with B7 | The six questions of §3.1 are answered on one screen for `ADMIN`, and for a view-only administrator |
| P5 | Tools home (§4), with B8 and the REST API screen | An entity's records are one search away; the page says which database and what the user may do |
| P6 | Monitor in other places (§2.5); the docs | Configuration has its Logging and telemetry tab, LLM and Agents its Traces, a service's page its activity; the docs name each moved screen by its one route |

P2 and P3 do not depend on each other. P4 needs both; P5 needs P3; P6 needs P1.

## 9. Decisions

Taken Nov 10, 2026:

| # | Decision | Taken |
| --- | --- | --- |
| D1 | Monitor as one section of System, or its screens spread through System's menu | One section, `/apps/system/Monitor/…`. What also belongs elsewhere is shown there too, by reuse (§2.5) |
| D2 | Move the Monitor files under `screen/System/`, or mount them where they are | Move them |
| D3 | `nav-card` as an XML element, or raw markup in each home | The element (B6) |
| D4 | The charts: an embedded shipped dashboard, or `m-chart` filled by the screen | The embed (B4) |
| D5 | Agent Evals | To System › LLM and Agents |
| D6 | The home screens' name | `Home`, file and route (§2.6) |
| D7 | The attention list | As §3.4, with resource use added: CPU load, memory, disk space above all, and what else the machine reports (inodes, open files, swap) |
| D8 | `MONITOR_APP` | Dropped; Monitor is granted with System (§2.1) |
| D9 | System › Data Document | To Tools › Data › Documents (§2.3) |
| D10 | The renames: Artifact summary, Hosted Instances, Git Repositories, Configuration, System Messages, Caches, Threads | Taken (§2.4) |
| D11 | `Monitor/dashboard.xml` | `Overview.xml`, Monitor's default (§2.1) |
| D12 | The `MONITOR_*` permissions (ingest, replay, incident, recover, settings, eval) | Kept: each gates one sensitive thing inside the section |
| D13 | The thresholds of §3.4 | As listed, with disk at 80 % and 90 % |

Nothing is open.

## 10. Later

Not in the phases above; each stands on its own. Built since, except the readers for Windows and macOS and an external
database's disk: see Filled after in [§11](#11-as-built).

- **Sparklines on counters**: a `sparkline` attribute on `number-card` (a list of numbers, drawn as the stat panel
  draws its own), so a landing's counters show their last hour without a chart row.
- **Meters in the Resources block**: a bar per row (`q-linear-progress` in the browser, block characters in the
  terminal) once the plain rows are in use.
- **Disk and memory on Windows, memory on macOS**: a reader without `unsafe` (a crate that wraps the system call, with
  its license and advisory check), since [the Rust practices](../../docs/dev/Practices-Rust.md) add no `unsafe`.
- **Certificate expiry** of the HTTPS listener as a reading, a gauge and an attention item.
- **The database's own disk** for an external Postgres, from the database, where its role may read it.
- **Drawer groups**: a `menu-group` on a screen or `subscreens-item`, drawn as a caption between entries, so a long
  flat menu like System's is grouped in the drawer as it is on the home.
- **A dashboard of the administrator's own** on the home: a setting naming the definition to embed in place of
  `system-home`.
- **Schema differences** on the Tools home: the count from the last Check Tables run, if a kept result makes it cheap.
- **Auto refresh** of the System home each minute, the metric documents' own pace; the wall display is Monitor ›
  Overview in kiosk mode today.

## 11. As built

The plan above stands as written, with these differences in the code.

**Data (P2).**

- The resource readings and their levels are in `moqui-core`'s `context/resources.rs`, not `context/status.rs`; the
  status map carries them as `Resources`.
- `routes` in `ec.factory.getStartupReport()` is a list of route texts; `tools` and `notBuilt` are lists of names.
- The three alert rule templates each have one threshold, the critical one of §3.4: disk above 90 % at once, memory
  above 90 % and CPU load above 100 % for five minutes. The warning level is on the System home only.
- An attention item is `{key, severity, text, count, value, link, parameters}`: `key` names the check, and `value` is
  a resource reading's percent where `count` is a count of records. The resource readings come first and the others
  after, each part with the most severe first. A count stops at 100 and reads "100 or more".
- The silent cluster member item has no link, since there is no members screen.

**System home (P4).**

- A `number-card` has one value and no second line, so a counter's second line (the share of the limit, the queue,
  those waiting) is in the card's title, with the level's word when it is at warning or critical.
- The Activity strip is rendered with the page, not in a `dynamic-container`, and has no link of its own to Monitor ›
  Overview: the embed's header has Open in Dashboards, and Monitor is the first featured card.
- The Cluster members list shows only when there is more than one member row: the Cluster listener is on for every
  server, so its state does not say the instance is clustered.
- The link groups are boxes in a `dashboard` grid, as many in a row as fit; on a phone they are one column and do
  not collapse to their titles.

**Navigation catalog (P4).**

- The rule is `template/HomeCatalog.js` (`build`, and `forScreen` over `sri.getMenuItems`); `template/HomeParts.xml`
  has the sections that show it: `NavFeatured` (cards of size `md`), `NavFeaturedLarge` (`lg`) and `NavGroups`.
- An entry is `{path, group, featured, text, links, title, icon, parameters, hidden}`, or `{url, title, …}` for a link
  the menus do not know (another app, a screen out of every menu). `title` and `icon` replace the menu item's;
  `parameters` is added to the link; `hidden` names a screen and shows nothing for it, so a tool whose permission
  the user does not hold is not in the last group either. A card's `links` are `{path | url, title, parameters}`.

**Tools home (P5).**

- The entity jump is one search with a Records or Definition choice beside it, not a second action on the row.
- The screen count is not counted in the render: `moqui.tools.ToolsHomeServices.get#ScreenCount` answers the count
  kept in the `tools.home.counts` cache, and when there is none or it is ten minutes old has `count#Screens` count
  again on a worker. Until the first count is kept the card says it is being counted. Both services are members of
  `TOOLS_APP`.
- The server-static report still names two things on the page: the database picker's form and the state chip of
  Running and recent.

**Monitor in other places (P6).**

- The `report-embed` behavior of §2.5, corrected there: a "Not available" notice, and the guard in the three Tools
  screens. The entity embed also needs the entity artifact type's metrics to be on.
- The `activity` panel of `monitor-artifacts` charts hits only, so a service's or an entity's page shows its hits
  over seven days; the duration is in Monitor › Artifacts, which the link beside the embed opens narrowed to it.

**Filled after (F1 to F6, Nov 10 and 11).** A comparison of the Java System and Tools screens with these, and of
this plan with the code, found what was broken or left, and §10 was built except its platform readers.

- **Tools, repaired.** Data › Export to the browser (it called a writer the data writer never had), its filter as a
  JavaScript object, Upload Snapshot (a stub), and the REST API screen, which links to the JSON documents: Swagger UI
  was never shipped and is not coming (Dev AIDE explores the APIs). A grid filter may begin with an operator
  ([Database](../../docs/tools/Database.md)), and an entity's row opens its record. Tables filters by rows; Schema ›
  Entities shows the view entities alone.
- **Threads.** A thread's native frames cannot be read from another thread without `unsafe`, so `moqui-core`'s
  `context/activity.rs` mirrors each thread's artifact stack where another thread reads it, and System › Threads
  shows what a thread runs, for how long, its user, its trace and that stack. Interrupt, the lock columns and the
  pool-size transition are gone.
- **Tools and Dev AIDE** link to each other's entity and service pages when Dev AIDE is mounted. basic-comms adds
  Voice and Chat Bridges to the System home with a `screen-extend`, and the Tools home is a clean server-static
  template.
- **Menu groups** (§10 drawer groups): `menu-group` on a `subscreens-item`, by default the screen's
  `default-menu-group`, a caption in the browser's drawer and a heading in the terminal's. The `html` navigation
  does not write it.
- **Counters and rows** (§10): `number-card` has `sparkline` (a line in qvt, bars in tui, nothing in html), filled on
  the System home by `get#CounterSeries`. The Resources meters and Auto refresh are `render-mode` texts and a client
  component (`m-page-refresh`), not new XML elements; Dashboards keeps its own refresh timer.
  `system_home_dashboard` names the home's dashboard. The Tools home shows the last Check Tables run of the process:
  what it changed and what failed, since a check applies the differences it finds.
- **Certificate expiry** (§10): a resource reading from the certificates in `runtime/cert`, with `resource-watch`
  thresholds (30 and 7 days), a gauge, an attention item and an alert rule template.
- **System messages.** A message whose outcome a person settled (Admin P13) stays Sending or Consuming, so the
  home's count of messages in error never had it; the failed and uncertain ones together are the Admin app's.
- **Not done here:** resource readers off Linux and an external database's disk (§10), a web script console (by
  decision), and the items of [Remaining-Work](../Remaining-Work.md#system-and-tools-apps).

