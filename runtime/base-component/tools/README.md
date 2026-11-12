# tools

The administration apps, mounted under `/apps` (and `/qapps`) by `MoquiConf.xml`: Dashboards, System and Tools. Monitor
([docs/tools/Monitoring.md](../../../docs/tools/Monitoring.md)) is a section of System, `/apps/system/Monitor`. On a
dev instance Dev AIDE ([dev-aide](../../dev-component/dev-aide/README.md)) is beside them.

Their access is three artifact groups granted to `ADMIN`: `TOOLS_APP` and `SYSTEM_APP` (`data/ToolsSecurityData.xml`)
and `DASHBOARDS_APP` (`data/MonitoringSecurityData.xml`). Monitor has no group of its own: the telemetry datasets and
its services and entities are members of `SYSTEM_APP`.

Another app may show the Dashboards screens under its own root, as BOSS Insights does
([Mounting the report screens](../../../docs/tools/Monitoring.md#mounting-the-report-screens)): its root sets
`reportLinks` (where each report link goes) and `reportPolicy` (the app's policy for the screens, as data), which
`template/ReportMount.js` reads for the screens, and Tools' own roots null. `test/screen/` holds the test mounts of the
ReportMounts and AdminMounts specs.

## Tools

`screen/Tools.xml`, grouped by section. The guide to Data, SQL, Schema, ETL and Database is
[docs/tools/Database.md](../../../docs/tools/Database.md); the design is
[plans/archive/Tools-Database.md](../../../plans/archive/Tools-Database.md).

| Screen | What it is |
| --- | --- |
| `Data/` | Grid (`Grid.qvue`: the data grid on an entity, a view, a data document, a SQL result or an uploaded CSV or spreadsheet, edited and saved in place, with saved views; a file has Load into…) and Record (one record's form), Browse (a record and its related records), Views (`DbViewEntity` views), Documents (DataDocument definitions and their feed indexing), Load, Export (entity data and data documents; the home of `DocumentExport.json`), Snapshots |
| `Sql.xml`, `Sql.qvue` | The SQL editor: statements run read only or with changes confirmed, results in a grid, explain, history and saved queries. `SqlTui.xml` is its terminal and plain-page form, `SqlDownload.xml` a result as XLSX. Needs `SQL_RUNNER_WEB` |
| `Schema/` | Entities and one Entity (with its table's differences), Check Tables, Indexes and keys (the model against the database, planned as jobs), Download DDL |
| `Etl.xml`, `Etl/` | ETL: Mappings, a Mapping (`Mapping.qvue`: source, target, fields with sample values, test, run, schedule), Runs and a Run (errors, error file, rerun), Value Maps; `Etl.xml` holds the JSON transitions the mapping form and the grid's Load into… use |
| `Database/` | Overview, Activity (sessions, blockers, index health), Queries, Tables (with maintenance), Jobs and a job's page, Settings, Speed Test, and Connections (external databases: `DATABASE_EXTERNAL`, never on production) |
| `Load.xml`, `Load/` | Load testing ([docs/tools/Load-Runner.md](../../../docs/tools/Load-Runner.md)): Scripts (the components' `test/load/*.load.js`), New run (`NewRun.qvue`, with `NewRun.tui.json`), Runs (with Compare and Import result), a Run (`Run.qvue`: live numbers and charts while it runs, with `Run.tui.json`) and Compare, over `org.moqui.dev.LoadServices`; `Load.xml` holds the JSON transitions the two `.qvue` files use, and `template/LoadRuns.js` the reads they share. Needs `LOAD_RUNNER` |
| `Service/`, `StatusFlows.xml` | Services (reference, detail, run) and status flows. A service's detail, an entity's (Schema) and Database › Overview show Monitor's numbers for it to a reader of the telemetry ([Monitor's screens in other places](../../../docs/tools/Monitoring.md#monitors-screens-in-other-places)) |
| `ArtifactStats.xml` | Artifact summary: the entities, services and screens counted by package. Its totals are the Tools home's counters. (Hit performance is Monitor › Artifacts) |
| `RestApi.xml` | REST API: the service REST resources and the entity master API, each linking to its description as JSON (a resource's OpenAPI document, an entity's master schema); `filter` narrows both lists, `api` (`service` or `entity`) shows one. There is no viewer: Dev AIDE explores them on a dev instance |

**Home** (`Tools/Home.xml`, [plans/archive/Tools-Fancy.md](../../../plans/archive/Tools-Fancy.md) §4) reads each block in its
actions:

| Block | Reads |
| --- | --- |
| Instance | `ec.factory.getStartupReport()` (the purpose; a production instance gets one banner line), `get#DatabaseList` and `get#Capabilities` of the selected database (kind and version) with the picker of `template/DatabaseParts.xml`, and `ec.user.hasPermission` for `SQL_RUNNER_WEB`, `DATABASE_ADMIN`, `DATABASE_EXTERNAL` and `LOAD_RUNNER` (a missing one is muted, its tooltip naming the permission) |
| Jump | Two server-search drop-downs over the `searchNames` transition (entity names, or the known service names, that hold the term; fifty at most). An entity opens in Data › Grid, or its definition; a service opens its detail, which has Run |
| Counters | The last Check Tables run of this process, when there was one (what it changed and what failed, kept by Schema › Entities in the `tools.home.counts` cache); `ec.entity.getAllEntityInfo`, `ec.service.getKnownServiceNames`, the EECA and SECA rule counts, the database size of `get#Capabilities`, and the screens from `moqui.tools.ToolsHomeServices.get#ScreenCount`, which answers the count it kept and has `count#Screens` count again on a worker (the walk loads every screen: about two seconds the first time in a process), so the page never waits for it |
| Main tools, More | The navigation catalog (`navCatalog`, below under System). SQL is left out without `SQL_RUNNER_WEB`, Load testing without `LOAD_RUNNER`; Dev AIDE is linked when its app is mounted |
| Pick up where you left off | The preference `tools.recent` (the entities the user opened last in Data › Grid, twenty names, `template/ToolsRecent.js`), `find#GridViews`, and with `SQL_RUNNER_WEB` `find#SavedQueries` and `find#SqlHistory`; ten rows, hidden when empty |
| Running and recent | `get#EtlRuns`, `get#DatabaseJobs` and, with `LOAD_RUNNER`, `find#LoadRuns`: five of each, newest first, hidden when empty |

Data › Grid takes `open=file` (the file dialog) and `open=views` (the saved views); SQL takes `queryId` and `auditId`
(a saved query or a statement of the user's history put in the editor, not run) and `open=history` or `open=saved`.

Data › Load and Snapshots can drop the foreign keys of the loaded tables first and make them again after: the load
runs on a worker as the `DatabaseLoadWrap` ServiceJob (`service/moqui/tools/DatabaseLoadServices.xml`, `data/ToolsDatabaseData.xml`).

The Data, SQL, Schema, ETL and Database screens take a `db` parameter, `group:<groupName>` (or `ext:<connectionId>` where an external database may be used), kept as the user
preference `database.selected`; `template/DatabaseParts.xml` has the picker.

## System

`screen/System.xml`: its screens are the files of `screen/System/`, so the file tree is the route.

- **The administration areas** (menu group Administration, and Integration for those of the integrations): top-down
  areas over the `org.moqui.admin` services, each a directory of sibling screens with relative links, which another app
  may show under its own root ([Mounting the administration screens](#mounting-the-administration-screens)). So far:
  **Access** (`System/Access/`): Groups and a group (GroupDetail: its profile, dated members, grants and defaults),
  Capabilities (the matrix of what the registered bundles and custom groups hold, `AccessServices.get#MatrixContext`),
  Row rules and the Access review for print and CSV (AccessReview, its rows made by `template/AccessReview.js`); and
  System's own Artifact groups and an artifact group (ArtifactGroupDetail), which no app shows. **Security**
  (`System/Security/`): Users (`UserAccount/`, System's own account administration, with the Verify screens of a user's own
  factors, which other apps mount by location), the Sign-in policy, Identity providers and a provider
  (IdentityProviderDetail) and Sessions. **Settings** (`System/Settings/`): the Instance settings (the registered
  preferences, and where each value of the configuration comes from) and Localization (the translations of messages and
  of registered labels). **Operations** (`System/Operations/`): Jobs, a job (JobDetail), Job runs, a run
  (JobRunDetail), Notification topics and a topic (TopicDetail). **Data** (`System/Data/`): Imports (the runs of every
  registered import target) and an import (ImportDetail: its rows, the result files, the retry of failed rows), Exports
  (the exports made, and the deliveries of scheduled reports and alerts as status), Export data (DataExport: the export
  of a registered format) and Backups, over `DataTransferServices` and `BackupServices`; the area itself links Tools'
  Data export, Data load and Snapshots for a user who opens Tools, and an import is staged and applied in a wizard an
  app mounts (the `dataImport` route), not here. **Audit** (`System/Audit/`): Changes (AuditChanges: the
  audit log's registered projections, `AuditServices.find#Changes`, each record linked to its place by the mount's
  routes). **Integrations** (`System/Integrations/`): Connections and a connection (ConnectionDetail: a
  gateway, a remote system, a mail sign-in provider, a printer), Message types and a type (MessageTypeDetail), System
  messages and a message (SystemMessageDetail). **Mail and protocols** (`System/Protocols/`): what each protocol is doing
  (Home), Mail relays and a relay (MailRelayDetail), the Mail queue and the Server configuration. The connection screens
  share `template/ConnectionParts.xml`. A screen's part for the instance administrator (a group's, a job's, a remote's,
  a message type's or a message's record changed directly, a group of any id and type created, a run or a message as it
  is stored, every produced message sent now, the visits by user, the stored translation rows changed directly, the
  whole audit log) shows only here. Where that part is a list of its own it is shown in place of the screen's list, by a
  parameter (`visits=Y` on Sessions, `rows=Y` on Localization, `log=Y` on Changes): two paginated lists on a page would
  share `pageIndex`.
- **Monitor** (`System/Monitor.xml`, `System/Monitor/`): the telemetry screens, Overview first
  ([docs/tools/Monitoring.md](../../../docs/tools/Monitoring.md#monitor)).
- **Configuration** (`System/Conf/`) shows the merged conf with the file each value came from (Settings), edits one
  conf file with `CONF_EDIT` (Edit, `Edit.qvue`, `EditTui.xml` in the terminal) and reloads it (Changes)
  ([docs/tools/Component-Deploy.md](../../../docs/tools/Component-Deploy.md)). Its Logging and telemetry tab
  (`Logging.xml`) is Monitor › Settings mounted a second time.
- **LLM and Agents** (`System/Llm/`): among its screens, Evals (agent evals) and Traces (`Traces.xml`, Monitor › AI
  mounted a second time).
- **Constitution** (`System/Constitution/`) reads and edits the instance's Constitution, previews its agent prompt
  section and manages the trusted reference origins ([docs/ai/Constitution.md](../../../docs/ai/Constitution.md)); its
  shared parts are `template/ConstitutionParts.xml`.

**Home** (`System/Home.xml`, [plans/archive/Tools-Fancy.md](../../../plans/archive/Tools-Fancy.md) §3) answers whether the instance is
healthy, what it is, what it has been doing, what is switched on, what needs an administrator and where to go. It
writes nothing, so it works for a view-only administrator; Auto refresh (`refresh=60` in the address) reads it again
each minute in the browser. Each block is read in its actions:

| Block | Reads |
| --- | --- |
| Identity and state | `ec.factory.getStartupReport()` (version, package, purpose, role, the time to ready) and `ec.factory.getStatusMap(true)` (host, OS, processors, start, `Ready` and `Draining`); the count of loaded components |
| Counters | The status map's `Resources` (the 5-minute load, memory, the fullest filesystem), `ec.factory.workerPool`, the `DataSources` pool numbers, and the users with a `moqui.server.VisitUserSummary` row today. A counter at a warning or critical level takes that color and says the level in its title. With telemetry on each has a sparkline of its gauge over the period (`get#CounterSeries`) |
| Activity | A `report-embed` of the shipped `system-home` dashboard, or of the one the `system_home_dashboard` setting names, over the `period` URL parameter (`1h`, `6h`, `24h`, `7d`), narrowed to this instance; one line in its place when telemetry is off or not in the build |
| Needs attention | `moqui.tools.SystemHomeServices.get#SystemAttention` (below) |
| Resources | The status map's `Resources` ([resource watch](../../../docs/Run-Deploy.md#resource-watch)): each reading with its level, a meter when it is a share, and a link to its series in Monitor › Metrics; a reading the platform does not report is "—" |
| Database | The `DataSources` by connection pool, `get#Capabilities` (kind, version, size) and the embedded database's backup job; the `moqui.server.ClusterMember` rows when there is more than one |
| Listeners and integrations | The runtime report: each listener and integration with its state and detail (failed and skipped first) and the screen that configures it, the tool factories, what the build leaves out, and the routes in a dialog |
| Navigation | The navigation catalog |

The navigation catalog is a list of entries in a home's actions (`navCatalog`), shown by the `NavFeatured`,
`NavFeaturedLarge` and `NavGroups` sections of `template/HomeParts.xml`; the rule is in `template/HomeCatalog.js`,
which describes an entry: one whose screen the menu does not show for the user has no tile, a `hidden` entry names its
screen and shows nothing, and a menu item no entry names is in a last group. A component adds entries with a
`screen-extend` `actions-extend` of the home, as basic-comms does for Voice and Chat Bridges. `HomeCatalog.js` also tells
whether Dev AIDE is mounted (`devMounted`): then an entity's and a service's page in Tools link to it in Dev AIDE.

`service/moqui/tools/SystemHomeServices.xml` has `get#SystemAttention`, the one list of what needs an administrator
(resource readings past their `resource-watch` thresholds, failed job runs, system messages in error and the like;
[plans/archive/Tools-Fancy.md](../../../plans/archive/Tools-Fancy.md) §3.4). `report/` holds the shipped dashboards, library panels
and alert rule templates: the `monitor-*` dashboards of Monitor, and in the System folder `system-home`,
`system-db-pool` and the disk, memory, CPU and certificate alert rules.

### Mounting the administration screens

Another app shows System's administration screens under its own root, as IronStone Admin does
([plans/Admin-Screens-Down.md](../../../plans/Admin-Screens-Down.md)). Its root sets `adminMount` in its
`always-actions`, so that the transitions read it too; `System.xml` sets it to null, so a request parameter never supplies
one. The map is data only, never authority:

| Key | What it does |
|---|---|
| `routes` | `{key: {url, idParameter, parameters, plain}}`: where a link to a record goes under the mount. The keys are the registrations' own (a recovery descriptor's or a transfer target's `ownerRoute`, a provider's or an audit projection's `linkKey`), one key for a screen, and the screens' own for a place that is not the framework's (`mailAccount`, `voice`, `supportSummary`, `dataImport`: the import wizard, by `targetId`). The framework's records have routes by default, to the System screens beside the one shown (`frameworkRoutes` in `template/AdminMount.js`: `adminUser`, `adminGroup`, `adminIdentityProvider`, `adminInstanceSettings`, `adminLocalization`, `adminMessageType`, `adminConnection`, `adminMailRelay`, `adminSystemMessage`, `adminJob`, `adminTopic`, `adminExports`, `adminImport`, `adminBackups`); a mount adds its apps' routes and may replace a framework one (`null` removes it). A route's `url` is a screen path relative to a screen of an area (`<Root>/<Area>/<Screen>`, where every such screen is shown), so it holds under any root, and the link widget disables it for a user its screen does not open to; with `plain: true` it is a URL used as it is, for another app's page (`/qapps/<App>/...`). A key with no route is no link |
| `hide` | The part keys the mount leaves out |

`template/AdminMount.js` answers where a screen stands: `state()` gives `{mounted, raw, routes, hidden}` in a screen's
actions, and `link(routes, key, id, more)` one record's `{url, parameters, plain, href}` (`more` is what else the link
carries; `href` is the URL with its query string). A screen shows it with two `link` widgets, one for each kind of route:
a plain one by its `href` (`url-type="plain"`), a screen path by its `url` and `parameters`. A link to a screen carries
what that screen declares as parameters of the linking screen's context too (a page index, a range of dates), so a list
that links another screen's record sends its own find values empty (AuditChanges), as a link that leaves a mode does
(`{visits:''}`).

`raw` is whether the instance administrator's parts are shown: what only System does (any field of a record, records no
registration covers, artifact authorization). They are never shown under a mount, and with none only to a user who opens
System by their own grant, asked of the user's own authorization records (`ec.artifactExecution.isPermittedOwn`), since an
app's inheritable always-allow on its area answers the stack's question for everyone and passes the transitions and entity
operations below it too.

So an app reuses a System screen in one of three ways:

- **By location**, a `subscreens-item` of its area pointing at the System screen, only when no grant of the app names
  the screen, the screen has no raw transition and the app adds nothing.
- **A wrapper** at the app's own location: `include-screen share-scope="true"` and one `transition-include` for each
  transition the app takes in. A raw transition that is not taken in does not exist on the app's route. This is the
  way for a screen with raw parts and for one a grant of the app names.
- **A screen of the app's own**, with its own actions and transitions and the System screen's named sections by
  `section-include`, where the app adds data of its own (an organization scope). A screen made for it says in its
  header comment which value the app sets and which sections it includes (Connections, Sessions, Imports, ImportDetail).

Every transition of a raw part carries `AdminMount.js`'s `raw()` as its `condition`. No group of the app is granted a
System screen's location: a grant there would open it under `/apps/system` too. The regression spec is
`test/AdminMounts.spec.js`, over the test mount `test/screen/AdminMount.xml`; it also reads every screen of the areas
(its `AREAS` list, which a new area is added to) and fails on a transition that acts and neither calls only
`org.moqui.admin` services nor carries `raw()`. Its `NOT_AREA_SCREENS` names what an area's directory holds that is not an
area's screen (Security's `UserAccount/`), and `UNGUARDED` a transition that acts and writes nothing, each with why.

## Tests

```sh
./moq test tools                      # the regression specs in test/ (budget in test/budget)
./moq accept tools                    # the acceptance specs in acceptance/
```

`test/load/` holds the shipped load scripts (`ServiceCall.load.js`: one service called again and again in this
server, so only Tools › Load runs it, not `./moq test-load`); `moqui test` does not read them. Writing and running
one: [docs/tools/Load-Runner.md](../../../docs/tools/Load-Runner.md).
