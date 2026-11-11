# System and Dashboards take the generic Admin and Insights screens

Plan and record of moving the framework-generic screens of IronStone Admin and Insights (mr-boss) into the framework's
System and Dashboards apps (`runtime/base-component/tools`), with BOSS mounting them and adding only its business parts.
Started Nov 11, 2026. The record of each phase is at the end.

Paths: `T` = `runtime/base-component/tools`, `F` = `runtime/base-component/framework`,
`B` = `runtime/component/mr-boss` (its own repository).

## Why

System was built bottom-up: raw record editors over entity-auto CRUD, ported from Moqui Java, for `ADMIN` only. BOSS
Admin and Insights were designed top-down and are nicer (state chips, phone columns, review before apply, refusals
shown as reasons), but much of what they show is framework data read through framework services (`org.moqui.admin.*`,
`org.moqui.report.ReportServices`) with no business model in it:

- 24 of Admin's 65 screens are framework-generic and 16 more are generic apart from an organization-scoped part, yet
  all live in mr-boss, so an instance without BOSS has only the old System screens.
- System and Admin each have a screen for jobs, job runs, system messages, message types, localization, groups and
  the audit log.
- Several report-engine features (dataset stewardship, scheduled delivery with audiences, business alerts, drill, the
  kiosk wall display) have their only screens in Insights.

Outcome: generic screens live in the framework's apps, BOSS mounts them and adds only its business parts, and each
record has one screen.

## Decisions

1. **System adopts Admin's areas** (Access, Security, Settings, Operations, Data, Audit, Integrations, Mail and
   protocols) with the same sibling screen names, so one file works under both mounts with its relative links.
2. **Replace, and fold the raw parts in.** The nicer screen becomes the only one; what only the old System screen
   could do becomes sections shown only to the instance administrator.
3. **The registries move to the framework, last** (reverses design-admin §14; droppable).
4. **Insights' engine screens move to Dashboards**: dataset stewardship, scheduled delivery, business alerts, and
   drill, wall display, share and Analysis.

## How BOSS reuses a tools screen

BOSS area grants are inheritable `AUTHZT_ALWAYS` (`B/data/BossSetupData.xml`, each `BOSS_ADM_<Area>`), which skips every
check below them: transition, entity-auto service, entity (`crates/moqui-core/src/user/authz.rs`, `is_permitted`). An
`ALWAYS` row naming a tools location would also open it at `/apps/system`, past the deny on `System.xml`. So:

| Kind | When | What it is |
|---|---|---|
| **D** direct mount | no BOSS authz row names the screen, it has no raw transition, and BOSS adds nothing | the BOSS area file's `subscreens-item` points at the tools file; the BOSS screen file is deleted |
| **W** thin wrapper | a BOSS row names the screen, or the tools screen has raw transitions | the BOSS file keeps its location: `include-screen share-scope="true"` plus one `transition-include` per transition BOSS takes in. A raw transition not taken in does not exist on the BOSS route |
| **S** BOSS screen | BOSS adds organization-scoped data or actions | own `actions` and transitions; widgets by `section-include` / `form extends` of the tools screen's named sections |

No `ArtifactGroupMember` row is re-pointed to a tools location: no BOSS group holds a grant on a Tools screen.

**Mount context** (the `reportLinks` pattern of [Monitoring](../docs/tools/Monitoring.md#mounting-the-report-screens)):
data only, set by the mounting root's `always-actions`, nulled by the tools roots.

- `adminMount` (nulled in `T/screen/System.xml`, set in `B/screen/Admin.xml`): `routes` `{key: {url, idParameter,
  parameters}}` keyed by the registrations' `ownerRoute` and `linkKey` values, framework keys defaulting to relative
  paths; optional `hide` list.
- `reportPolicy` (beside `reportLinks`): `features {alert, share}`, `range`, `datasetIdPrefix`,
  `datasetScopeParameter`, `recipientPermissionId`, `reportEmailTemplateId`, `alertAudience`, `shareGroups`,
  `recipientCap`, `calendar`. New `reportLinks` keys `alerts`, `alertShared`, `wallDisplay`; `drill` gains a Tools
  default.
- `T/template/AdminMount.js`: `state()` gives `{mounted, raw, routes}`, `raw()` is a transition's condition. `raw` is
  true when there is no mount and the user opens System by its own grant. Every raw transition carries the condition.
- Capabilities are `ec.user.hasPermission('ADMIN_*')` and the services' own refusals. `AdminCalls.js` (`read`,
  `reasonOf`) is the framework's (`F/service/org/moqui/admin/AdminCalls.js`). Links to System-only screens are screen
  paths (`//apps/system/...`), which the link widget disables for a user who may not follow them.

## Track A: Admin to System

### Target System menu

| Idx | Group | Area | Screens (new unless noted) |
|---|---|---|---|
| 1, 2 | | Home, Monitor | unchanged |
| 3 | Administration | Access | Groups, GroupDetail, Capabilities, RowRules, AccessReview, ArtifactGroups, ArtifactGroupDetail (moved, System only) |
| 4 | Administration | Security | UserAccount/** (unchanged path, with Verify*), SignInPolicy, IdentityProviders, IdentityProviderDetail, Sessions |
| 5 | Administration | Settings | InstanceSettings, Localization; SettingsHome in A6 |
| 6 | Administration | Operations | Jobs, JobDetail, JobRuns, JobRunDetail, Topics, TopicDetail; Health, SupportSummary in A6 |
| 7 | Administration | Data | Backups, Exports, DataExport, Imports, ImportDetail; links to Tools › Data Export, Load, Snapshots |
| 8 | Administration | Audit | AuditChanges |
| 9 | Administration | Visit | unchanged path |
| 10, 11 | Integration | Integrations, Protocols | Connections, ConnectionDetail, MessageTypes, MessageTypeDetail, SystemMessages, SystemMessageDetail; Home, MailRelays, MailRelayDetail, MailQueue, ServerConfiguration |
| 12-18 | Integration | EntitySync, EdgeNode, EdgeClients, Voice, ChatBridges | unchanged |
| 20-42 | Server, AI, Content, Print and hosting | Components, Conf, Cache, ThreadList, TerminalSessions, Llm, Constitution, Resource, Print, Instance | unchanged |

Deleted, folded in: `System/ServiceJob/**`, `System/SystemMessage/**` (remotes into ConnectionDetail, kind remote),
`System/Security/UserGroup/**`, `Security/ActiveUsers` (into Sessions), `System/Localization/**`, `System/AuditLog.xml`.
One route per screen: links, the home's `navCatalog`, `SystemHomeServices`, BOSS fix paths, cargo tests, prompts and
skills are changed; no aliases are kept.

### Per screen

| Area | Screen | Raw sections folded in (instance administrator only) | BOSS |
|---|---|---|---|
| Operations | Jobs | scheduler box (read only) | D |
| | JobDetail | any `ServiceJob` field, parameters, notification users, run now | W |
| | JobRuns | | D |
| | JobRunDetail | unredacted parameters, results, errors | D |
| | Topics, TopicDetail | | W (`BOSS_ADM_OPERATIONS_TOPICS`) |
| Integrations | Connections | | S: organization filter |
| | ConnectionDetail | any `SystemMessageRemote` field, enum maps | S: `PaymentGatewayOrganization` box |
| | MessageTypes, MessageTypeDetail | create a type, any field | W |
| | SystemMessages, SystemMessageDetail | send/consume all, new incoming, update, edit text, cancel, reset, ack | W; owner link from `routes` |
| Mail | MailRelays, MailRelayDetail | | D |
| | MailQueue, ServerConfiguration | | W (`BOSS_ADM_PROTOCOLS_OPS`) |
| | Home | | S: readiness box, Comms links |
| Access | Groups | create any group | W |
| | GroupDetail | any field, permissions, artifact authz, tarpits, preferences, any-user membership | S: member organization scope |
| | Capabilities, RowRules | | D (matrix services move down) |
| | AccessReview | | S: member scope |
| Security | SignInPolicy, IdentityProviders, IdentityProviderDetail | | D |
| | Sessions | visits by user | S (`Users/Sessions`: Mantle identity read) |
| Settings | InstanceSettings | | W (`BOSS_ADM_ORGANIZATIONS`; Settings has no area group) |
| | Localization | create/update/delete any row | W |
| Data | Backups | | W (`BOSS_ADM_DATA_BACKUPS`) |
| | Exports, DataExport, Imports, ImportDetail | | S: transfer check, organization filter |
| Audit | AuditChanges | all audit rows (old AuditLog list) | D if `find#Changes` alone carries the scope, else S |

Moving down: `B/template/AdminConnectionParts.xml` to `T/template/ConnectionParts.xml`; `getUserList` to
`T/template/UserChoiceTransitions.xml`; `CapabilityServices.get#MatrixContext`/`apply#MatrixChanges` to
`org.moqui.admin.AccessServices` (`AccessMatrix.js`); the generic parts of `Data.js` to `DataTransferExports.js`.
Deleted: `get#AppAccess`, `find#ToolLinks`, BOSS `AdminCalls.js`.

Stays in BOSS: Admin Home, LogNote, Organizations, OrganizationDetail, the Users screens, ReferenceData, Devices (every
call is Mantle's), Constitution (already shares `ConstitutionParts.xml`), AccessLog (a BOSS dataset with an organization
row filter), Monitoring and AgentsHome (landings into Monitor and Llm), the `Shared/DataImport` wizard. Admin › System
(the whole-app mount for `ADMIN`) stays. Admin's Developer tools area is removed: Tools Home's catalog lists the tools
by permission. Nothing moves to Tools: System's Data area holds the transfer screens and links Tools' raw Export, Load
and Snapshots.

### Phases

Each phase is a mr-moqui commit, then a mr-boss commit; both repositories work after each.

| Phase | mr-moqui | mr-boss | Specs |
|---|---|---|---|
| A0 Contract | `AdminCalls.js` in framework; `AdminMount.js`; `System.xml` nulls the map; test mount `T/test/screen/AdminMount.xml` | `Admin.xml` sets `adminMount`; one route table; screens use the framework `AdminCalls`; screen-path links; delete `get#AppAccess`, `find#ToolLinks`, the Developer area | `T/test/AdminMounts.spec.js`; `AdminAccess`, `AdminJobRuns` |
| A1 Operations | `System/Operations/**` with raw sections; delete `ServiceJob/**`; home links, `navCatalog`; cargo paths | area items re-pointed; fix paths | `AdminMounts` (raw transition refused under a mount, allowed under System); `AdminJobRuns`, `AdminRegistries`, `SystemAttention` |
| A2 Integrations, Mail | `System/Integrations/**`, `System/Protocols/**`, `ConnectionParts.xml`; delete `SystemMessage/**` | D/W/S per the table; mr-marble's SystemMessageDetail link | `AdminSystemMessages`, `AdminRelays`, `AdminMailQueue` |
| A3 Access, Security | `System/Access/**`, `AccessMatrix.js`, `Security/Sessions` and the sign-in screens; delete `UserGroup/**`, `ActiveUsers` | area files; matrix services deleted | `F/test/AdminAccessCapabilities`; `AdminGroups`, `AdminSecurity`, `AdminCapabilities` |
| A4 Settings, Audit | `System/Settings/**`, `System/Audit/**`; delete `Localization/**`, `AuditLog.xml` | wrappers | `AdminSettings`, `AdminAudit` |
| A5 Data | `System/Data/**` | area file, S screens, Backups wrapper | `AdminExports`, `AdminImports` |
| A6 Registry (after track I; droppable) | `moqui.server.ReadinessProvider`, `RefreshOperation`, `SettingsEntry`; `org.moqui.admin.ReadinessServices`; framework seed rows; `get#SystemAttention` becomes the not-ready checks of `get#Readiness`; `Operations/Health`, `SupportSummary`, `Settings/SettingsHome` | entities and most of `Registry.js` deleted; the Mantle adapters become contract services the rows name (as People's `people.setup` is already); rows re-seeded | a framework registry spec; `SystemAttention`, `AdminRegistries`, `PeopleAccess` |

## Track I: Insights to Dashboards

Tools gets the generic screens, each reading and writing only through `ReportServices`, with presentation scripts in
`T/template/Report*.js`. A mountable screen never reads an entity directly.

| Dashboards screen | Change | Insights after |
|---|---|---|
| Datasets, DatasetDetail, DatasetFields, DatasetView | merge: the business list and stewardship on top; today's telemetry and index list stays as "Other datasets" where there is no mount | W (grants name locations and transitions) |
| Schedules, ScheduledReportDetail, ReportDelivery | new; `Reports.xml` keeps previews, the compose link and the owner-mode form | D; the area adds the recipient cap |
| Alerts, AlertRule, AlertShared | one list (Mine, Received, Templates, Kind) and one editor: a shared basis form, then Contact points or Audience; links from `reportLinks`; state through `get#AlertState` | W taking in only the business transitions |
| Drill, Analysis | new generic screens | S: scope, meaning, record links, cost filter; organization calendar |
| Dashboard | `share` transition and dialog (owner only; own groups plus `reportPolicy.shareGroups`) | W |
| Playlists, WallDisplay | new list, create and share screen; the hardened kiosk player | S (devices, display share); W |
| Explore, Split, Panels | the "no readable dataset" guard and default range move into Explore | D |

New `ReportServices` operations (`F/service/org/moqui/report/ReportReads.js`): `get#BusinessDataset`,
`get#BusinessDatasetOptions`, `find#ReportRecipients`, `find#ScreenSchedules`, `store#ScreenSchedule`; `ownerName` in
the answers of `get#ReportOccurrence` and `get#AlertState`. Five artifact rows are added to existing BOSS groups.

Stays in BOSS: the Insights root, Home, the Reports catalog, Records, Flows, Settings, AddToDashboard, SavedReports,
ManagementDigest, and the wrappers Dashboards, DashboardEditor and PanelDetail.

| Phase | mr-moqui | mr-boss | Specs |
|---|---|---|---|
| I0 Contract | `reportPolicy` nulled at `Dashboards.xml` and `System/Monitor.xml`; `ReportMount.js`; Explore guard; Monitoring.md | root sets `reportPolicy`; direct-mount Panels, Explore, Split | `ReportMounts`, `InsightsAccess` |
| I1 Datasets | two operations; four screens; `ReportDatasets.js` | four wrappers; two rows | `BusinessDatasets`; `InsightsDatasets` |
| I2 Delivery | recipient and screen-schedule operations, `ownerName`; three screens; `Reports.xml` trimmed | items re-pointed; three rows | `ReportAudience`; `InsightsDelivery`, `InsightsSettings` |
| I3 Alerts (after I2) | merged list and editor; `ReportAlerts.js` | two wrappers; `Alerts.js` deleted | `AdminOverview`; `InsightsAlerts` |
| I4 Drill, Analysis, share, playlists, wall | five screens | wrappers; `DashboardServices` deleted | `InsightsRecords`, `InsightsDashboards`, `InsightsWallDisplay` |

## Order and tests

A0 to A5, I0 to I4, A6, then a finishing phase (docs, the acceptance specs changed by path, run one by one).

- Per phase: the focused specs named above and `moqui lint --screens` on the changed screens; the full
  `./moq test-framework` and `./moq test-apps` once at the end.
- Each moved screen's one render stays the BOSS spec's render through the Admin or Insights route; the raw sections
  are rendered by the cargo integration tests (`crates/moqui-host/tests/it/tools_screens.rs`). Tools gains one spec
  file, `AdminMounts` (transition posts only), within its budget.
- Every trim has a negative spec: a raw transition posted under a mount is refused for a holder of the delegated
  permission and allowed under System; the same parameters succeed for the positive case.

## Record

### A0 Contract (Nov 11, 2026)

Built: `ec.artifactExecution.isPermittedOwn` (`moqui_core::user::permitted_own`: the user's own authorization records,
nothing from the stack, no hit counted and no failure recorded); `framework/service/org/moqui/admin/AdminCalls.js`
(moved from mr-boss unchanged); `tools/template/AdminMount.js` (`state`, `raw`, `link`); `System.xml` nulls `adminMount`;
the test mount `tools/test/screen/AdminMount.xml` with its probe, and `tools/test/AdminMounts.spec.js`. In mr-boss:
`get#SharedContext` answers `mount` and `Admin.xml` sets `adminMount` from it; the screens read through the framework
`AdminCalls`; Jobs, JobRunDetail, Monitoring and Home ask `isPermittedOwn` of an app's root in place of `get#AppAccess`;
`get#AppAccess`, `find#ToolLinks`, the Developer tools area and `BOSS_ADM_DEVELOPER` are removed (Home links Tools for
a user whose own grant opens it).

Findings:

- F1. `raw` is asked with a host member, not `sri.buildUrl(...).getDisableLink()`: the question must hold in a transition's
  condition and under any root (the test mount's root is not webroot, where `//apps/system` does not resolve), and
  `isPermitted` answers true under any inheritable always-allow on the stack, which is what an app's area grant is. The
  probe's `stack=true raw=false` pins the difference.
- F2. A value of `adminMount` that is not a map (a request parameter under a root that set none) is a mount with nothing
  in it: no route is taken from it and nothing raw is shown, so a third mount that forgets the map fails closed.
- F3. `tools/test/LoadRunner.spec.js` failed in the baseline run on a decimal's `valueOf`. The fix made here (comparing
  the status map's `MemoryUsed` by identity) did not hold; A1 has the real one.

Tests: `./moq test tools` before the phase, 3.2 s of the 4 s budget; `./moq test tools --spec AdminMounts` (2 features,
0.02 s); `./moq test mr-boss --spec AdminAccess --spec AdminJobRuns --spec AdminRegistries` (16 features);
`cargo lint`; the typegen golden rewritten with `UPDATE_GOLDEN=1` and read. The acceptance specs
`AdminFoundationWalk` and `AdminRegistriesWalk` changed (the Developer area and the two removed services) and are run in
the finishing phase.

### A1 Operations (Nov 11, 2026)

Built: `System/Operations.xml` and its six screens, Admin's with their BOSS references gone and the instance
administrator's parts folded in: Jobs (the scheduler's state), JobDetail (RawJob: any field of the job, any parameter, the
users notified, a direct run; eight transitions, each with `raw()` as its condition), JobRunDetail (RawRun: the run as
stored, and its logs in Monitor; trace and log windows around the run), JobRuns, Topics, TopicDetail. `System/ServiceJob/**`
is deleted; `getUserList` is `template/UserChoiceTransitions.xml`. System's top-level menu is renumbered and regrouped to
the target layout. Links, the home's catalog, `get#SystemAttention`, the job-error topic's link template, the docs and
the cargo tests name the new paths. In mr-boss: Jobs, JobRuns and JobRunDetail are mounted by location and their files
deleted; JobDetail, Topics and TopicDetail include the System screens and take in `saveJob`, `runJob`, `saveDefaults` and
`saveSubscription`; the registry's job links and fix paths go to Admin › Operations (so they open to the operations
operator, not only to ADMIN under Admin › System).

Findings:

- F4. The route of a failed item's system message came out disabled under BOSS's specs: they render with `Admin.xml`
  as the root, where a `//Admin/...` path does not resolve. A mount's routes are relative to a screen of an area, as the
  framework's own are, which holds under any root.
- F5. The LoadRunner failure of F3 was not the `==`: a spec condition's parts are shown by their values, and a decimal
  has no `valueOf`, so `memoryMb > 0` on a decimal threw. The comparison is now made on plain numbers and the condition
  is one boolean.
- F6. `tools/test/AdminOverview.spec.js` held the display rules of the old job run screen's script (outcome words from
  the raw record). The new screen's outcome is the service's (`framework/test/AdminJobs`); the feature now holds the
  screen's own rules: the trace and log windows and an item's link by the mount's route.
- F7. `get#SystemAttention`'s failed-runs link carried the old list's find parameters (`hasError`, `startTime_from`);
  it now sends `state=failed` and `fromDate`, which the new list reads.

Tests: `./moq test tools` (70 features, 3.2 s of 4 s; `AdminMounts` now also posts JobDetail's `updateJob` under the
test mount, refused for the delegate and the instance administrator, and under System, applied, a forged `adminMount`
ignored); `./moq test mr-boss --spec AdminJobRuns --spec AdminRegistries --spec AdminAccess` (16 features; AdminJobRuns
posts `updateJob` through Admin › Operations as the operator and as ADMIN: the job is unchanged);
`cargo test -p moqui-llm a_plural_or_a_suffix`; the slow `cargo test -p moqui-host --test it tools_screens -- --ignored`
(changed: it renders every System screen and the detail paths as ADMIN, so the raw parts too; 24.5 s);
`moqui lint --screens` with no parse failure. Changed and not run: the opt-in `tui_screens` cargo test (its job walk
now uses JobDetail's "Run directly now" and "Runs") and the acceptance `AdminJobsUsable` and `AdminRegistriesWalk`.
