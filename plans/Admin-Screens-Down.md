# System and Dashboards take the generic Admin and Insights screens

Plan and record of moving the framework-generic screens of IronStone Admin and Insights (mr-boss) into the framework's
System and Dashboards apps (`runtime/base-component/tools`), with BOSS mounting them and adding only its business parts.
Started Nov 11, 2026. Every phase (A0 to A6, I0 to I4) is built; the record of each phase is at the end, the
finishing phase last.

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
3. **The registries move to the framework, last** (reverses design-admin §14; done in A6).
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
  parameters, plain}}` keyed by the registrations' `ownerRoute` and `linkKey` values, framework keys defaulting to
  relative paths; optional `hide` list.
- `reportPolicy` (beside `reportLinks`; nulled in `T/screen/Dashboards.xml` and `T/screen/System/Monitor.xml`, set in
  `B/screen/Insights.xml`): `features {alert, share}`, `range`, `datasetIdPrefix`,
  `datasetScopeLabel`, `datasetScopeHelp` (I1), `recipientPermissionId`, `reportEmailTemplateId`, `alertAudience`, `shareGroups`, `recipientCap`, `calendar`, each
  described in [Monitoring](../docs/tools/Monitoring.md#mounting-the-report-screens) (`datasetScopeParameter` left the
  contract in I3: a scope policy registers its own). `reportLinks` has the keys `alerts` and `alertShared` since I0,
  read by the alert screens since I3, when `alertRule` became where a rule opens for any reader and `features.alert`
  what offers setting one; `playlists`, `wallDisplay`, `analysis` and a Tools default for `drill` came with their
  screens (I4).
- `T/template/ReportMount.js`: `state()` gives `{mounted, links, policy}`, every key and field present, and
  `url(key, own)` is a transition's response. It is the one reader of both maps. `policyWith(fields)` is the root's
  policy with an area's fields, which the area sets as `reportPolicy` in its own `always-actions` (I2: the recipient
  cap); with no mount it is null.
- `T/template/AdminMount.js`: `state()` gives `{mounted, raw, routes, hidden}`, `raw()` is a transition's condition. `raw` is
  true when there is no mount and the user opens System by its own grant. Every raw transition carries the condition.
  `link(routes, key, id, more)` is one record's link, and `systemPlace(key, id, more)` a framework route as System's
  own absolute place, for the home's attention service (A6).
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
| 5 | Administration | Settings | InstanceSettings, Localization, SettingsHome (A6) |
| 6 | Administration | Operations | Jobs, JobDetail, JobRuns, JobRunDetail, Topics, TopicDetail, Health and SupportSummary (A6) |
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
| | Health | | S: the organization as a filter and scope, a fix link only where it opens, its own `requestRefresh` (the record's A6) |
| | SupportSummary | | D: no grant names it, its one transition is delegated, Admin adds nothing (the record's A6) |
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
| | SettingsHome | | W (`BOSS_ADM_SETTINGS_INDEX`: every BOSS user's) |
| Data | Backups | | W (`BOSS_ADM_DATA_BACKUPS`) |
| | Exports, DataExport | | D: Admin's own part was the permission check the services make (the record's A5) |
| | Imports | | S: organization filter |
| | ImportDetail | | S: the import read with the wizard's context |
| Audit | AuditChanges | all audit rows (old AuditLog list) | D: `find#Changes` alone carries the scope (the record's A4) |

Moving down: `B/template/AdminConnectionParts.xml` to `T/template/ConnectionParts.xml`; `getUserList` to
`T/template/UserChoiceTransitions.xml`; `CapabilityServices.get#MatrixContext`/`apply#MatrixChanges` to
`org.moqui.admin.AccessServices` (`AccessMatrix.js`); the generic parts of `Data.js` and `DataImport.js` to
`DataTransferExports.js` and `DataTransferImports.js`; the three registry entities and most of `Registry.js` to
`moqui.server` and `org.moqui.admin.ReadinessServices` (`Readiness.js`, A6).
Deleted: `get#AppAccess`, `find#ToolLinks`, BOSS `AdminCalls.js` (the framework's now), `find#ChangeEvidence` (A4),
`ironstone.admin.DataServices` (A5).

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
| A4 Settings, Audit | `System/Settings/**`, `System/Audit/**`; delete `Localization/**`, `AuditLog.xml` | two wrappers, AuditChanges by location; `find#ChangeEvidence` deleted | `AdminSettings`, `AdminAudit` |
| A5 Data | `System/Data/**`; five `DataTransferServices` operations | area file, two screens by location, two S screens, Backups wrapper; `DataServices` deleted | `F/test/AdminDataExport`, `AdminDataTransfer`; `AdminExports`, `AdminImports` |
| A6 Registry (after track I) | `moqui.server.ReadinessProvider`, `RefreshOperation`, `SettingsEntry`; `org.moqui.admin.ReadinessServices`; the framework's own checks and settings entries as seed rows; `get#SystemAttention` lists the instance's checks of `get#Readiness` that are not ready beside its own readings; `Operations/Health`, `SupportSummary`, `Settings/SettingsHome`; `AdminReadiness.js` | entities and most of `Registry.js` deleted; the Mantle adapters are one contract service the rows name, `check#OwnerReadiness` (People's `people.setup` names Mantle's own); rows re-seeded with route keys; Health S, SupportSummary D, SettingsHome W | `F/test/AdminReadiness`; `SystemAttention`, `AdminMounts`, `AdminRegistries`, `AdminAccess`, `PeopleAccess`, `InsightsSettings` |

## Track I: Insights to Dashboards

Tools gets the generic screens, each reading and writing only through `ReportServices`, with presentation scripts in
`T/template/Report*.js`. A mountable screen never reads an entity directly.

| Dashboards screen | Change | Insights after |
|---|---|---|
| Datasets, DatasetDetail, DatasetFields, DatasetView | merge: the business list and stewardship on top; today's telemetry and index list stays as "Other datasets" where there is no mount | W (grants name locations and transitions; the record's I1) |
| Schedules, ScheduledReportDetail, ReportDelivery | new; `Reports.xml` keeps previews, the compose link, the playlists (until I4) and the owner-mode form, and links Schedules | D; the area adds the recipient cap (the record's I2) |
| Alerts, AlertRule, AlertShared | one list (All where rules have contact points, Mine, Received, Templates; Kind) and one editor: a shared basis form, then Contact points or Audience; links from `reportLinks`; kind and state through `get#AlertState` | W for the list and the editor, taking in only the business transitions; AlertShared not shown (the record's I3) |
| Drill, Analysis | new generic screens over `ReportDrill.js` and `ReportAnalysis.js` | S: scope, meaning, record links, cost filter; the admitted datasets, the organization's calendar and Insights' own `saveAnalysis` (the record's I4) |
| Dashboard | `share` transition and dialog (owner only; own groups plus `reportPolicy.shareGroups`, with no mount too) | W, `share` taken in |
| Playlists, Playlist, WallDisplay | new list, create and share screen; the player in the app, as it was; the hardened kiosk page | S (devices, display share) with Playlist included when one is chosen; W |
| Explore, Split, Panels | the "no readable dataset" guard moves into Explore and Split, the default range into Explore (and Dashboard); Panels carries its menu image | D (the record's I0) |

New `ReportServices` operations (`F/service/org/moqui/report/ReportReads.js`, and `ReportWrites.js` for the write):
`get#BusinessDataset`, `get#BusinessDatasetOptions`, `find#ReportRecipients`, `find#ScreenSchedules`,
`store#ScreenSchedule`, `find#ShareGroups` (I4); `ownerName` in the answers of `get#ReportOccurrence` (I2) and
`get#AlertState` (I3: its own code, `alert_delivery.rs`); the group rule of `save#Definition` (I4: `definitions.rs`).
Six artifact rows are added to existing BOSS groups, and one group for the wall displays' share artifact.

Stays in BOSS: the Insights root, Home, the Reports catalog, Records, Flows, Settings, AddToDashboard, SavedReports,
ManagementDigest, and the wrappers Dashboards, DashboardEditor and PanelDetail.

| Phase | mr-moqui | mr-boss | Specs |
|---|---|---|---|
| I0 Contract | `reportPolicy` nulled at `Dashboards.xml` and `System/Monitor.xml`; `ReportMount.js`, which every Dashboards screen reads the links through; Explore and Split guard; Monitoring.md | root sets `reportPolicy`; direct-mount Panels, Explore, Split | `ReportMounts`, `InsightsAccess` |
| I1 Datasets | two operations; four screens; `ReportDatasets.js`; a scope policy registers its parameter; `create#BusinessDataset` asks the named dataset's update artifact | four wrappers; two rows; `DatasetServices` keeps readiness and refresh | `BusinessDatasets`; `InsightsDatasets` |
| I2 Delivery | recipient and screen-schedule operations, `ownerName`; three screens; `ReportDelivery.js`; `policyWith`; `Reports.xml` trimmed | items re-pointed, the area adds the cap; three rows; `Delivery.js` keeps the digests and what the alerts use | `ReportAudience`, `ReportMounts`; `InsightsDelivery`, `InsightsSettings` |
| I3 Alerts (after I2) | merged list and editor; `ReportAlerts.js`; `ownerName` in `get#AlertState`; a panel's New alert rule by `features.alert`; `datasetScopeParameter` gone | two wrappers, the area adds the cap; `Alerts.js` and `AlertServices.xml` deleted; no row | `BusinessAlerts`, `AdminOverview`, `ReportMounts`; `InsightsAlerts`, `InsightsAccess` |
| I4 Drill, Analysis, share, playlists, wall | four screens and Dashboard's share; `find#ShareGroups`; the group rule of `save#Definition`; `ReportShare.js`, `ReportPlaylists.js`, `ReportDrill.js`, `ReportAnalysis.js`; `Reports.xml` gives up the playlists | Dashboard and WallDisplay wrappers, Playlists, Drill and Analysis around the Tools sections; `DashboardServices` deleted; one row and one group | `ReportAudience`, `ReportMounts`; `InsightsDashboards`, `InsightsWallDisplay`, `InsightsRecords` |

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

### A2 Integrations, Mail (Nov 11, 2026)

Built: `System/Integrations.xml` (Connections, ConnectionDetail, MessageTypes, MessageTypeDetail, SystemMessages,
SystemMessageDetail) and `System/Protocols.xml`, "Mail and protocols" (Home, MailRelays, MailRelayDetail, MailQueue,
ServerConfiguration): Admin's screens with their BOSS references gone, and `template/ConnectionParts.xml` (moved from
mr-boss unchanged but for its header). The instance administrator's parts folded in, each transition with `raw()` as its
condition: SystemMessages (RawMessages: send all produced, consume all received, a message typed in), SystemMessageDetail
(RawMessage: the stored record, its text edited in the `EditMessageText` subscreen and downloaded, send, consume, reset to
produced, send ack, reset from error, cancel, the messages it acknowledges and those split from it, status history and
errors), MessageTypes (RawTypes: create a type), MessageTypeDetail (RawType: any field), ConnectionDetail for a remote
system (RawRemote: any field of the `SystemMessageRemote`, its enumeration values created, updated and deleted).
`System/SystemMessage/**` is deleted. `AdminMount.js`: a route may be a plain URL (`plain: true`, another app's page) and
`link(routes, key, id, more)` answers `{url, parameters, plain, href}`; `systemMessage` is the new detail screen. The
owner links of SystemMessages, SystemMessageDetail and MailQueue, the protocol rows of Home and ServerConfiguration's
support summary go by the mount's routes (`ownerRoute` keys, `mailAccount`, `voice`, `supportSummary`). The home's catalog,
`get#SystemAttention`, the cargo tests and the UI-Refinement inventory name the new paths. In mr-boss: MailRelays and
MailRelayDetail are mounted by location and their files deleted; MessageTypes, MessageTypeDetail, SystemMessages,
SystemMessageDetail, MailQueue and ServerConfiguration include the System screens and take in `saveType`,
`recoverMessages`, `recoverMessage`, `retryOutcome`, `settleOutcome` and `runCheck`; Connections (the organization filter
and column, by `form extends`), ConnectionDetail (the `PaymentGatewayOrganization` box) and Protocols › Home (the Comms
links, Voice and the transport health box) are Admin's own screens around the System screens' named sections; the mount
adds `storeIntegration` and `mailAccount` (plain) and `supportSummary`; the registry's backup-mirror fix path and the
transports card go to Admin › Integrations. mr-marble's OrderDetail links the new message screen.

Findings:

- F8. `get#SystemAttention` counts every message in error, and the new list shows its last 14 days unless it is told from
  when: the item's link sends `state=failed` and `fromDate`, the day of the oldest message it counted (as F7 did for the
  job runs).
- F9. `getUserList` (`template/UserChoiceTransitions.xml`, A1) searched every account with no `raw()` condition. The new
  static rule found it once it read a `transition-include` at its source; it carries the condition now.
- F10. Three places the screens link are not the framework's: a mail account's own screens (`mailAccount`), the phone
  system's (`voice`) and a support summary (`supportSummary`, until A6). They have no framework route, so System shows no
  link there; BOSS gives `mailAccount` and `supportSummary` in its mount, and its Protocols › Home sets `voice` from the
  settings index for a viewer who opens it. basic-comms' System › Voice is not linked from System's protocol list: tools
  does not name another component's screen, and a component cannot add a route.
- F11. A remote system is started from Connections (the `remote` kind is `creatable`, `store#Connection`), so the raw
  part has no "create message remote"; the old screens had no delete of a remote and none is added.
- F12. The generic MailQueue cannot say "Mail account, in Comms": a mail account's owner reads "Mail account", linked by
  the mount's route. Connections' heading no longer names BOSS's administrators.
- F13. The BOSS sources had `always-actions` after the transitions (ConnectionDetail, MailRelayDetail) and `align` on a
  `default-field` (MailQueue), both outside the XSD; the System screens and BOSS's ConnectionDetail have them in order.
- F14. mr-boss's acceptance sweeps list Admin's screens from its own files (`AdminSpecLib.sweepScreens`), so the screens
  A1 mounted by location had left them; the list now takes the System screens an area file mounts by location too.

Tests: `./moq test tools` (71 features, 3.2 to 3.3 s of 4 s; `AdminMounts` gains the static rule over `System/{Operations,
Integrations,Protocols}`: every transition that acts, a `transition-include` read at its source, calls only
`org.moqui.admin` services or carries `raw()`; and a plain route's link on the probe);
`./moq test mr-boss --spec AdminSystemMessages --spec AdminRelays --spec AdminMailQueue --spec AdminRegistries --spec
AdminJobRuns --spec AdminAccess` (22 features; AdminSystemMessages posts `updateMessage` through Admin › Integrations as the
operator and as ADMIN: the message is unchanged); the slow `cargo test -p moqui-host --test it tools_screens -- --ignored`
(the new detail paths, a remote's ConnectionDetail and a relay added to its data; 23.9 s) and
`cargo test -p moqui-host --test it system_messages_and_entity_syncs_run_over_rest` (its two renders are the new list and
detail); `moqui lint --screens` with no parse failure and no flagged site in the new screens. Changed and not run: the
acceptance `AdminRegistriesWalk` (the fix path) and the sweeps' screen list in `AdminSpecLib.js` (`AdminTui`,
`AdminServerStatic`).

### A3 Access, Security (Nov 11, 2026)

Built: `System/Access.xml` (index 3, tabs, Groups first) with Admin's Groups, GroupDetail, Capabilities, RowRules and
AccessReview, their BOSS references gone, and `Security/ArtifactGroup/**` moved in flat as ArtifactGroups and
ArtifactGroupDetail (System's own: each of their eleven writing transitions carries `raw()` now). The instance
administrator's parts folded in, each transition with `raw()` as its condition: Groups (RawGroups: a group of any id and
type created directly), GroupDetail (RawGroup, what the User Groups screens did: any field of the group; a permission of
any id applied, an existing one applied, a new one made, its end date changed, removed; a preference of a registered or
any key; an artifact authorization and a tarpit created, changed and deleted, their forms extending ArtifactGroupDetail's
as before; any user's membership with its dates; sixteen transitions and `getUserList`). `System/Security/` gains
SignInPolicy, IdentityProviders, IdentityProviderDetail and a new Sessions (every account's sessions over
`IdentityServices.find#Sessions` with `allUsers`, the bulk sign-out through `revoke#Sessions`; RawVisits, the Active
Users screen's visits by user, shown in place of the list with `visits=Y`); its menu is Users, Sign-in policy, Identity
providers, Sessions, Users the default, and it stays a popup panel (its first item is System's old-style Users section, a
container of screens the terminal tests walk; nothing of the mount depends on the panel). `Security/UserGroup/**`,
`Security/ArtifactGroup/**` and `Security/ActiveUsers.xml` are deleted. The matrix services moved down:
`AccessServices.get#MatrixContext` and `apply#MatrixChanges` (`AccessMatrix.js`, over `Access.js`'s `lib`, which gains
`requireManage`), granted in `SecurityTypeData.xml` beside the other access services and in
`docs/dev/JS-Framework-Services.md`. The review's rows are `tools/template/AccessReview.js` (`view`), which the System
screen calls with every member named and mr-boss's `get#AccessReview` with the users in scope. The home's catalog (the
Administration group, the Users and access card's links, Users today), `UserAccountDetail`, `EntitySyncDetail`, the
gallery's settings link, the prompts and skills (`small.md`, `tools-json.md`, `openui-lang.md`, `vue-sfc.md`), the agent
task files, the cargo tests and the UI-Refinement inventory name the new paths. In mr-boss: Capabilities, RowRules,
SignInPolicy, IdentityProviders and IdentityProviderDetail are mounted by location and their files deleted (no grant
names them, they have no raw transition, Admin adds nothing); Groups includes the System screen and takes in only
`createGroup`; GroupDetail (the group as an Admin record, the members in scope by `get#MemberScope`, the choice of a
member and `check#Record` before a membership), AccessReview (`get#AccessReview`) and Users › Sessions (Mantle's
`find#Users`, the organization filter, its own `revokeSessions`) are Admin's own screens around the System screens' named
sections; Security › Devices is untouched; the matrix services and what only they used are gone from
`CapabilityServices.xml` and `Capabilities.js`; `mountRoutes` needed nothing new (a group's page is the same relative path
under both roots, a user's is the `user` route).

Findings:

- F15. `org.moqui.admin` has no service that lists accounts for a delegate, so the generic AddMember's choice of a user
  is `getUserList`, the instance administrator's: under System that is whoever opens it, and an app has a members box of
  its own (BOSS: `get#Choices`, then `check#Record`).
- F16. A form inside a field of an extended form is the other screen's: BOSS's GroupMembers, extending the System
  screen's, failed with "Could not find form EndMember", so it overrides that field with its own End dialog. The rest of
  the list and AddMember extend cleanly.
- F17. A link to an alias transition carries every declared parameter of its target that is in the context:
  ArtifactGroupDetail's link to a group has GroupDetail's four parameters in its static template now (the gate3
  expectation follows).
- F18. `find#Sessions` could not narrow every account's sessions by a name: it takes `text` (username or full name,
  ignoring case, in the query; `framework/test/AdminSessions`). The visits by user replace the sessions list rather than
  stand beside it, since two paginated lists on a page share `pageIndex`.
- F19. The static rule found SignInPolicy's `reviewRule` (actions that only carry the form's values to the review):
  named in `UNGUARDED` with why. `Security/UserAccount/**` is named in `NOT_AREA_SCREENS`: System's own account
  administration and the Verify screens apps mount on purpose; every other screen of Security is read.
- F20. The Groups list shows a group by its description in the `userGroupId` field and its id in `userGroupIdText`: what
  an agent's read of the list answers. The canvas tests (`it/assist.rs`, `gate3_browser.rs`) and the `vue-sfc` skill's
  example read those fields, with `text` as the find parameter.
- F21. The recorded agent replays drove the User Groups screens by more than a path (the list's `where`, "Create User
  Group", "User Group ID", "Update"), so a change of paths alone did not replay: with the task files on the new screens
  15 of 24 tasks of `replay/` and 7 of 9 of `replay-small/` passed. The 9 and 2 recordings that named
  `Security/UserGroup` are written again by hand for the new screens, each with the steps it had: a read of the Groups
  list by `text`; the "New custom group" dialog's Identifier and Description (CreateGroup) for the hand-off; Name and
  Find for the follow-up; and for the writes GroupDetail's "All settings" form (Description, "Update group",
  `updateUserGroup`), since a group a task makes has no type, is neither a registered bundle nor a custom group, and so
  has no Profile form. That form is below the rows a screen answer shows, so in the five write recordings the model
  first moves to it (`find` "Update group", which shows its Description field and button) and sets and presses in the
  next call, one model call more than each had. Their `user`, `model` and `tool` lines are the transcripts' of the
  replay that played them, the headers kept. Both sets pass (24 of 24, 9 of 9) and the rewritten recordings replay with
  no drift; the task files' checks are unchanged. The baselines were stale before this phase: at the commit before it
  all 24 passed and the run still failed against `baseline-replay.json`, every task's tokens 32 to 78 percent over the
  baseline's (22 to 78 percent over in `replay/` and 67 to 83 in `replay-small/` with the rewritten recordings). Both
  are written again from the passing replays (`--write-baseline`, at the user's word), and each set then replays with
  exit 0.
- F22. The opt-in `gate3` and `gate3_browser` tests had failures before this phase that are not its own: three of
  `gate3` (goldens of SYSTEM_APP's and TOOLS_APP's members, an html page read with its chunk size) and four of
  `gate3_browser` (the gallery's dialog and tray, Dev AIDE's service writer and screen builder), the same before and
  after; and the slow `qvt_report` test fails on `Tools/Service/ServiceDetail.xml`'s `report-embed` after the fixture's
  own checks pass.

Tests: `./moq test tools --nobuild` (71 features, 3.3 s of 4 s; `AdminMounts` reads Access and Security in its static
rule and posts GroupDetail's `updateUserGroup` under System, applied); `./moq test framework --nobuild --spec
AdminAccessCapabilities --spec AdminAccessServices --spec AdminSessions` (13 features, 0.4 s; the matrix's own rules moved
here from mr-boss in one feature, 0.08 s; in two runs of thirteen AdminAccessServices' "a membership change is seen at once by
the session it changes" failed its last condition, a membership ended at the moment it is asked about, and passed again); `./moq test mr-boss --nobuild --spec AdminGroups --spec AdminSecurity --spec
AdminCapabilities --spec AdminAccess --spec AdminUsers --spec AdminDevices --spec AdminRegistries` (30 features;
AdminGroups posts `updateUserGroup`, `createUserGroupPermission` and Groups' `createUserGroup` through Admin as the access
administrator and as ADMIN: nothing changed); `cargo test -p moqui-host --test it tools_screens -- --ignored` (the new
detail paths, a sign-in provider added to its data; 21 to 25 s) and, of the same binary, `the_canvas_over_http_on_postgres`,
`the_drawer_calls_over_http_on_postgres`, `xlsx_spreadsheets_images_and_ipp_printing` and `types`;
`cargo test -p moqui-llm --lib` (338, the skills' examples among them); `cargo test -p moqui-host --test gate3` (7 pass,
the 3 of F22 fail as before) and `--test gate3_browser` (25 pass, the 4 of F22 fail as before);
`moqui test tools --agents --replay` on both sets (F21); `moqui lint --screens` with no parse failure and no flagged
site in the new screens, `moqui lint` of the changed service files. Changed and not run: the acceptance
`AdminFoundationWalk`, `AdminReviewWalk` and `AdminCapabilitiesUsable` of mr-boss and the framework's
`acceptance/client/LoadBasic`.

### A4 Settings, Audit (Nov 11, 2026)

Built: `System/Settings.xml` (index 5, tabs, InstanceSettings first) with Admin's InstanceSettings and Localization,
their BOSS references gone, and `System/Audit.xml` (index 8, tabs) with Admin's AuditChanges, which now reads
`org.moqui.admin.AuditServices.find#Changes` itself (through `AdminCalls.read`, so a refusal shows as its reason), makes
the texts before and after of a masked value, and links a record by the mount's route for the change's `linkKey` with
its `linkId`. The instance administrator's parts folded in: Localization (RawRows, what the Localization screens did:
the stored rows of `LocalizedMessage` and `LocalizedEntityField` found, created, changed and deleted directly; six
transitions, each with `raw()` as its condition) and AuditChanges (RawLog, the Audit Log screen's list: every row of the
audit log, read only, nothing listed until something is asked for). Each is shown in place of the screen's own list, by
a parameter (`rows=Y`, with `kind` for messages or labels; `log=Y`), as Sessions' visits are (F18). InstanceSettings has
no such part; its link to the conf is System's Conf, for a user who opens System by their own grant.
`System/Localization.xml`, `System/Localization/**` and `System/AuditLog.xml` are deleted. `AdminMount.js`'s framework
routes are the linkKey values of the framework's audit projections that have a System screen: `adminUser` (System's own
account page), `adminGroup`, `adminIdentityProvider`, `adminInstanceSettings`, `adminLocalization`, `adminMessageType`,
`adminConnection`, `adminMailRelay`, `adminSystemMessage`, `adminJob` and `adminTopic`; the exports, imports and backups
get theirs in A5, and what only an app has (invitations, lifecycle operations, organizations) is a mount's. The home's
catalog (the Administration group gains Instance settings, Localization, Audit changes and Visits; the "Security and
history" group and the two Localization entries of Content are gone), the gallery's settings link, the framework's
comments that named the old screens, the cargo tests and the UI-Refinement inventory name the new paths. In mr-boss:
InstanceSettings and Localization include the System screens at their own locations (`BOSS_ADM_ORGANIZATIONS` names
both) and take in `valueOptions`, `storePreference`, `storeMessage` and `storeLabel`; AuditChanges is mounted by
location and its file deleted (no grant names it, it has no transition, Admin adds nothing: F25);
`ironstone.admin.AuditServices.find#ChangeEvidence` and what only it used are gone (`Audit.js` is the access log's); its
`recordRoutes` are `mountRoutes` in `Admin.js`, as relative routes, for what differs from the framework's or is Admin's
own (`adminUser`, `adminInvitations`, `adminLifecycle`, `adminOrganization`, `adminExports`, `adminImport`,
`adminBackups`). Settings' other screens and AccessLog are untouched; Constitution links neither removed screen.

Findings:

- F23. One key for a screen. The mount's routes had `user` and `systemMessage`, keys of the screens' own, and the audit
  registrations name the same places `adminUser` and `adminSystemMessage`. The registrations' keys are data in two
  components, so the two older keys are renamed to them, in `AdminMount.js`, Sessions, GroupDetail, JobRunDetail, the
  test mount and its probe, the AdminOverview spec and mr-boss's `mountRoutes`.
- F24. A link carries what its target declares as parameters of the linking screen's context, and an empty value in the
  link's own parameters is what leaves one out. Three places: Sessions' link back from the visits (A3) kept `visits=Y`
  and never left them, fixed with `{visits:''}`; AuditChanges' "Show all" and its narrowing links kept the user, record
  or session they were to drop, as they did in mr-boss, fixed in `clearValues`; and a record's link from the changes
  list would carry the list's page and dates to a target that declares the same names (Exports, Imports, Connections,
  Localization), so it sends the list's own find values empty.
- F25. AuditChanges holds mounted by location. `find#Changes` checks `ADMIN_AUDIT_VIEW` and finds an organization's
  records through their owner within the caller's scope, by a direct call too (mr-boss's AdminAudit asks it as the audit
  reader: its own organization's person found, the other's not found or counted). What mr-boss added is held so: only an
  Admin user opens the route (the area's grant) and the service refuses without the permission; a record's place is a
  screen path, which the link widget disables for a user that screen does not open to: the audit reader, who holds no
  other area, sees each record named in a disabled anchor in html (`href="#"`, `aria-disabled`) and as muted text in
  qvt, with no path in the page, and the administrator sees links to Access, Users and Settings; the sentence that named
  whose changes are shown is one caption true under both roots ("what belongs to the whole instance, and what belongs to
  an organization within those you administer"). ADMIN through Admin sees no part of the instance administrator's.
- F26. The screen lint reads a name that ends in Rows as a collection and flags its truthiness, so Localization's flag
  is `showStored`.
- F27. InstanceSettings' "Open the conf" is a screen path to System's Conf (`//apps/system/Conf`), shown when the
  user's own records open System, as JobRunDetail's Monitor links are. Under a spec's root the path does not resolve
  and the link renders disabled; the cargo test, on a server, holds its text for ADMIN, and mr-boss's AdminSettings that
  the configuration administrator has none.
- F28. The stored rows' list takes the screen's `locale`, which is also the find's, so it is narrowed by a locale chosen
  among the translations, and after a row is created or deleted directly it shows that row's locale.
- F29. mr-l10n holds translations of strings only the removed screens and the old catalog showed ("Audit Log", "New
  Message", "New Field L10n", "Entity Fields", "Localized messages", "Localized entity fields", "Security and
  history"), and none of the moved screens' strings (Admin is not in the pack). It is not changed here.
- F30. No recorded agent replay names a removed screen. Both sets replay (24 of 24, 9 of 9, exit 0 against the
  baselines, which are not written again); none of their drift notes is about these screens.

Tests: `./moq test tools --nobuild` (71 features, 3.3 s of 4 s; `AdminMounts` reads Settings and Audit in its static
rule, at least 34 delegated and 56 raw transitions, and posts Localization's `createLocalizedMessage` under System,
applied); `./moq test framework --nobuild --spec AdminAuditChanges --spec AdminConfiguration` (6 features, 0.3 s);
`./moq test mr-boss --nobuild --spec AdminSettings --spec AdminAudit --spec AdminAccess --spec AdminRegistries --spec
AdminGroups --spec AdminUsers --spec AdminJobRuns --spec AdminSecurity` (31 features; AdminSettings posts
`createLocalizedMessage` and `deleteLocalizedMessage` through Admin as the configuration administrator and as ADMIN:
nothing created, nothing deleted; AdminAudit's one render holds the masked text, the reader's organization and the
disabled record link); `cargo test -p moqui-host --test it tools_screens -- --ignored` (the stored rows of both kinds,
the whole audit log and the conf link's text added; 25 s); `moqui test tools --agents --replay` on both sets (F30);
`moqui lint --screens` with no parse failure and no flagged site in the changed screens, `moqui lint` of the two changed
service files. Changed and not run: mr-boss's acceptance `AdminAuditUsable` (System's audit path among what stays closed
to the audit reader; the record link present for the administrator and absent for the reader) and the sweeps' screen
list comment in `AdminSpecLib.js` (`AdminTui`, `AdminServerStatic`).

### A5 Data (Nov 11, 2026)

Built: `System/Data.xml` (index 7, tabs, Imports first, Backups for a user who reads only the backups) with Admin's
Imports, ImportDetail, Exports, DataExport and Backups, their BOSS references gone. The area itself links Tools' Data
export, Data load and Snapshots (`//apps/tools/Data/Export`, `Load`, `Snapshots`) for a user whose own grant opens
Tools: the links are the area file's, so an app that shows the screens under its root does not show them. No screen of
the area has an instance administrator's part: there was no System screen over transfers or backups to fold in, and the
entity data of the instance itself is Tools'. What Admin's `ironstone.admin.DataServices` and the list of
`ironstone.shared.DataImportServices` did moves to the framework as five operations of
`org.moqui.admin.DataTransferServices`, in the modules of the parts they read (`DataTransferExports.js`,
`DataTransferImports.js`), granted in the `AdminTransferServices` group (`AdminConfigurationData.xml`) as the others
are and described in `docs/dev/JS-Framework-Services.md` and `docs/security/Attack-Surface.md`: `find#ExportRuns` (a
page of the export runs with their counts as text, and the targets), `get#ExportContext` (the export wizard's state),
`run#ExportRequest` (a wizard's form values as `run#Export`'s request; `ADMIN_TRANSFER_RUN`), `find#ImportRuns` (a page
of the import runs with their counts as text, and the targets) and `get#ImportDetail` (one import with its target's
words, its scope by name and its rows as text). They answer an owner's place as the registration's key, never a URL.
The screens link it by the mount's routes: a target's or a run's `ownerRoute`, with the run's own parameters, and a new
key of the screens' own, `dataImport`, for the import wizard an app mounts (the targets started in it from Imports, an
open import continued from ImportDetail); with no mount a target started in a wizard is only named. ImportDetail has
its own `results` transition (`get#ImportReport`) in place of the wizard's. `AdminMount.js`'s framework routes gain
`adminExports`, `adminImport` and `adminBackups`. The home's catalog has Imports, Exports and Backups in the
Administration group. In mr-boss: Exports and DataExport are mounted by location and their files deleted; Backups
includes the System screen at its own location (`BOSS_ADM_DATA_BACKUPS` names it) and takes in `runBackup`; Imports
(the organization as a filter, `get#Choices`, and as the scope of `find#ImportRuns`; the column's name) and
ImportDetail (the import read by `get#ImportContext`, the wizard's own read under Admin's mount, with the organization
by its name where the target's owner names its organizations itself) are Admin's own screens around the System screens'
named sections, ImportDetail taking in `retry` and `results`; `DataServices.xml` and `Data.js` are deleted, and
`find#ImportRuns` is gone from `DataImportServices`; the two tables of owners' places (`Data.js`, `DataImport.js`) are
`mountRoutes` entries, as plain routes, with `dataImport`, and `adminExports`, `adminImport` and `adminBackups` are
dropped there (the framework's are the same relative paths). The shared wizard reads its "imported in its owner's own
wizard" link from the mount's routes too (`get#ImportMount` answers them under Admin), so no second table is left.

Findings:

- F31. There is no check of a transfer as an Admin record. What `Data.js` and `DataImport.js` asked under Admin was
  `get#SharedContext`'s `transferView` and `transferRun`, which are `ADMIN_TRANSFER_VIEW` and `ADMIN_TRANSFER_RUN` for a
  user of Admin: the area's grant decides who opens the route and the framework's operations ask the two permissions
  themselves, so Admin adds nothing to Exports and DataExport. No grant names them and they have no raw transition, so
  they are mounted by location, not kept as screens of Admin's own as the plan's table had it (corrected). For the same
  reason ImportDetail takes in the System screen's `retry` rather than keeping one that asks `get#ImportMount` first.
- F32. `DataTransferExports.js` was already the framework's export module, so the wizard's and the list's projections
  are added to it and to `DataTransferImports.js` rather than being a new module; they call the module's own functions,
  and `run#ExportRequest` calls `run#Export` as a service, for the transaction of its own that the record and the audit
  row of an export are written in.
- F33. The sentence at the bound of one export named BOSS ("Larger volumes are not a BOSS export"); the framework's
  says "Larger volumes are not exported here", with the rest as it was (ask a system administrator, who has tools for
  volume), on DataExport and in a refusal's reason. Exports' two sentences that named Insights say whose a report is
  and that a target exported in its owner's place is opened there; the owner's app is in the targets list.
- F34. The static rule reads a transition's `service-call` elements, so ImportDetail's `results` calls
  `get#ImportReport` by a `service-call` and sends the file in a script after it (the wizard's own calls it from a
  script, which the rule would name as acting past the delegated services).
- F35. A link to ImportDetail carried the Imports list's page (both declare `pageIndex`), and the link back carried the
  import's target as the list's filter; each sends the other screen's value empty (F24). The Imports parts send the
  find values of the screen showing them empty with an owner's link, an app's own among them (the organization).
- F36. A field of an extended form-list is replaced whole by a field of the same name: Admin's Imports names the scope
  column Organization so. ImportDetail's label for the scope is the target's registered `scopeLabel`, or what the
  screen showing it sets.
- F37. The reads of Backups, Exports, DataExport and Imports go through `AdminCalls.read`, so a refusal is shown as
  its reason after the sentence that it is not available ("Backups are not available to you: ADMIN_BACKUP_VIEW is
  required."); ImportDetail keeps the one sentence, since an import that is not the user's and one that is not there
  are not told apart.
- F38. No recorded agent replay names a screen or a service of this phase (not replayed).

Tests: `./moq test tools --nobuild` (71 features, 3.3 s of 4 s; `AdminMounts` reads Data in its static rule: four
transitions, all delegated, so at least 38 delegated and 56 raw; no post added); `./moq test framework --nobuild --spec
AdminDataExport --spec AdminDataTransfer` (6 features, 0.3 s; the wizard's and the lists' rules moved here from
mr-boss: one new feature of AdminDataExport over the spec's own target, and the import list and detail in
AdminDataTransfer's last feature; `AdminBackups`, which this phase does not change, was run beside them five times and
failed once, at its lock check, and passed the other four); `./moq test mr-boss --nobuild --spec AdminExports --spec AdminImports --spec
ProductsImport --spec AdminAccess --spec AdminRegistries --spec AdminAudit` (AdminExports keeps the contact book's
export through Admin's route, the status list and the three renders, and holds an owner's place as its key in the
answer and as its URL in the page; AdminImports the same for an owner's run, a wizard target and the shared wizard's
link); `cargo test -p moqui-host --test it tools_screens -- --ignored` (the area's Tools links, a seeded import's
detail with its retry, the export wizard's review of the framework's own target, Exports and Backups; 26 s);
`moqui lint --screens` with no parse failure and no flagged site in the changed screens, `moqui lint` of the two
changed service files. Changed and not run: mr-boss's acceptance `AdminReviewWalk` (the export service's name) and the
sweeps' screen list comment in `AdminSpecLib.js` (`AdminTui`, `AdminServerStatic`).

### I0 Contract (Nov 11, 2026)

Built: `tools/template/ReportMount.js`, the one reader of a report mount's two maps: `state()` answers `{mounted,
links, policy}`, every link key present (null where not offered) and every policy field with its default, and
`url(key, own)` is a transition's response (the mount's screen, else the screen's own default, else Tools'). The seven
Dashboards screens that read `reportLinks` themselves (Home, Dashboard, Definition, Explore, Datasets, Reports,
Playlist; twenty places) read through it. `reportPolicy` is the second map: `Dashboards.xml` and `System/Monitor.xml`
null it with the other four names, and the fields are described in Monitoring.md, whose mount section also has the
table of which report screen an app shows how. Explore and Split say that the viewer reads no dataset, and query
nothing, when none is readable and none is asked for (by id, or for Explore by a saved search); Explore, its tail and
Dashboard with its panel export and inspect open on the mount's range when the request names none. Panels has a menu
image. The link keys `alerts` and `alertShared` are in the contract with Tools' own screens as their defaults; no
screen reads them yet. In mr-boss: `get#Context` answers `reportPolicy` and `Insights.xml` sets it beside
`reportLinks` (a wall display's request gets the range alone); Panels, Explore and Split are mounted by location and
their three files deleted; `reportLinks` has `alerts` (the Rules list) and `alertShared` (null); `InsightsAccess` holds
the reader's and the author's policy and the framework's guard on Explore's one render; the acceptance sweeps take the
screens mounted by location from `InsightsSpecLib.mountedScreens`.

What each field of `reportPolicy` stands for in Insights today, its value there, and its phase:

| Field | What Insights does today | Insights' value | With no mount | Used from |
|---|---|---|---|---|
| `features.alert` | `Alerts.js` lets a holder of `INSIGHTS_AUTHOR` or `INSIGHTS_DISTRIBUTE` set a rule; the same two decide `reportLinks.alertRule` | the two permissions | true | I3 |
| `features.share` | Dashboard's `shareable` and `share#Dashboard` need `INSIGHTS_AUTHOR` | the permission | true | I4 |
| `range` | `get#Context`'s defaults, which the Explore and Dashboard wrappers put in `from` and `thru` | `now-30d` to `now` | `now-7d` to `now` | I0: Explore, Dashboard |
| `datasetIdPrefix` | `Datasets.js` `createdPrefix` | `insights-` | none | I1 |
| `datasetScopeParameter` | the dataset forms' "Organization field", sent as `rowFilterParameters.organizationField` (Mantle's scope policy's parameter) | `organizationField` | none asked | I1 |
| `recipientPermissionId` | `Delivery.js` `readerIds`: only holders of `INSIGHTS_VIEW` are proposed or accepted as recipients, of a schedule and of an alert | `INSIGHTS_VIEW` | the engine's own rule | I2, I3 |
| `reportEmailTemplateId` | `Delivery.js` `TEMPLATE`, sent with every `store#Schedule` | `BOSS_INSIGHTS_REPORT` | the engine's | I2 |
| `alertAudience` | `Alerts.js` `TOPIC`, `TEMPLATE` and `DETAIL_LINK`, written into every rule in place of contact points | the topic `InsightsBusinessAlert`, the template `BOSS_INSIGHTS_ALERT`, the link to AlertDetail | contact points | I3 |
| `shareGroups` | `get#ShareGroups` adds the wall displays' group to the sharer's own groups, and `share#Dashboard` accepts it | `BOSS_INSIGHTS_DISPLAY`, "Wall displays" | none | I4 |
| `recipientCap` | `Insights.js` `recipientCap`: the organization's setting under the instance ceiling, sent with every schedule, audience check and alert rule | not set by the root (F44) | the engine's | I2, I3 |
| `calendar` | `Insights.js` `calendarParameters`: the organization's display zone and week start as `time_zone` and a week bucket's `offset`, in Analysis' queries | those of the organization in scope, null where it set none | the viewer's zone, the instance's week | I4 |

No field of the plan's list is left out: each has a present use in Insights.

The three wrappers, and where what each did is now:

| Wrapper | What it did | Now |
|---|---|---|
| Panels | included Tools' Panels and took in Home's `dashboard`, `star`, `edit`, `new` and `explore` | Tools' Panels takes them in itself |
| | set `definitionType` to panel | Tools' Panels sets it |
| | menu title, index 3 and image | the `subscreens-item` in `Insights.xml` (title, index); the image is Tools' Panels' (F46) |
| Explore | took in all nine transitions of Tools' Explore | on the route by location |
| | `from` defaulted to Insights' range | `reportPolicy.range`, read by Explore (`thru` too) |
| | the "no dataset" sentence for a viewer who reads none and asks for none, in place of the screen | Tools' Explore, with no mount too |
| | menu index 1 under Report builder | the `subscreens-item` in `ReportBuilder.xml` |
| | declared only `datasetId` and `savedSearchId` as its parameters | Tools' Explore declares all of its own (F45) |
| Split | included Tools' Split, hidden from the menu | the `subscreens-item`, `menu-include="false"` |

Findings:

- F39. Insights' root grant is an inheritable allow for its users (an always-allow only for `ADMIN`), and
  `BOSS_INS_REPORT_ENGINE` denies them every `ReportServices` operation that a capability group is not granted. So a
  Dashboards screen mounted by location is opened by the root and each operation it calls is still checked: the rule for
  a direct mount holds more easily here than under Admin's area grants. No row of `BossSetupData.xml` names Panels,
  Explore or Split or a transition of one (the nearest, `BOSS_INS_LEGACY`'s old patterns over Dashboards and
  ReportBuilder, are plain names since P4 and match nothing), nothing includes the wrappers by location, and every link
  to them is by path, which is unchanged.
- F40. There was no `Report*.js` in `tools/template`; the links were read in place, each with Tools' default beside it.
  The module now holds each key's default once, and `ReportMounts`' static rule fails for a screen under
  `screen/Dashboards` or `screen/System/Monitor` that reads `reportLinks` or `reportPolicy` from the context itself.
  mr-boss's own screens still read `reportLinks` directly (Playlists, Home), as their own map.
- F41. There is a mount when either map is set. A value that is not a map (a request parameter under a root that set
  none) is a mount with nothing in it, as F2: no link is offered, and neither feature. Under a mount a feature is
  offered only when the policy says so; every other field a mount leaves out has Tools' value.
- F42. The guard holds with no mount too, where a user who reads no dataset was shown Tools' log dataset's refusal.
  Its first sentence is the one Explore already had ("No dataset is available to you yet.", which mr-l10n
  translates), followed by how a dataset comes to be read. Split has the same guard; its ranges keep their own
  defaults (a week and the week before), since Explore sends its range when it opens Compare.
- F43. Dashboard reads its default range from the policy as well, so the contract's rule (the policy a mount sets is
  what a screen under it reads; a forged one is ignored under Tools) is carried by the two dashboard renders
  `ReportMounts` already had, and no render is added. Insights' Dashboard wrapper still sets `from` and `thru` itself,
  to the same values; I4 can drop those two lines. Playlist's three `now-7d` defaults are not changed (I4).
- F44. The root does not set `recipientCap`. Reading it is a call for the instance ceiling and the settings of the
  active organization, or of up to 50 organizations for an administrator of all with none active, on every Insights
  request. The plan already has the Scheduled reports area add it (I2); the Alerts area does the same in I3. The
  `calendar` is that of the organization in scope at the root (the active one, or the only one); a screen with an
  organization choice of its own (Analysis) sets its own.
- F45. A screen mounted by location declares all of its parameters on the app's route. Explore declares twenty, the
  wrapper declared two, so a link to it carries more of the linking screen's context (F24): the Report builder's tab
  to Explore from DatasetView carried the dataset and now carries that screen's range, query and `pageIndex` too
  (seen in the menu's data), so Explore opens on the same page number of its own list. Tools' own menu does this
  between Dashboards screens. Not changed here: the choice is this or a wrapper kept only to declare less; I1 should
  settle it when DatasetView is rewritten.
- F46. A `subscreens-item` has no menu image, so an item mounted by location shows the screen's own. Tools' Panels has
  Insights' image now, and is the only item of the Dashboards menu with one.
- F47. The Insights sweeps list its screens from its own files, as Admin's did (F14): `InsightsSpecLib.mountedScreens`
  reads the items of the root and the areas that name a Dashboards screen, for `InsightsTui` and
  `InsightsServerStatic`.
- F48. The policy's fixed values (the dataset prefix, the alert topic, the two templates, the detail link) are written
  twice in mr-boss until I1 to I3 delete the modules that use them: in `Insights.js` `reportPolicy` and in
  `Datasets.js`, `Delivery.js` and `Alerts.js`.
- F49. For I1: `datasetScopeParameter` is a name only. The form's label and help ("Organization field", "The field
  that carries each row's organization") are Insights' words, and with no mount the framework's own scope policy
  takes `groupField`, Mantle's `organizationField`: the parameter belongs to the policy chosen, not to the app, so I1
  may do better to read it from the scope policy's registration.
- F50. For I2 to I4: `recipientPermissionId`, `shareGroups` and `recipientCap` are data a Tools transition would pass
  on. They cannot be forged (the roots set or null the map), but what they allow must be decided by the service called:
  the engine's `save#Definition` stores the group it is given without asking whether the saver belongs to it
  (`definitions.rs`), where `share#Dashboard` refuses a group that is neither the sharer's own nor the wall displays'.
- F51. Explore's `tail` and `remember` fall back to Tools' log dataset when a request names no dataset, by location
  as under the wrapper; the page always sends one, and the engine refuses the rest.
- F52. No recorded agent replay names a screen or a sentence of this phase (not replayed).

Tests: `./moq test tools --nobuild` (72 features, 3.3 s of 4 s; `ReportMounts` has three: the mounted dashboard's
existing render now also holds the mount's range over a forged one, a new transition of the test mount answers
`state()` under it, Tools' dashboard render holds Tools' range over a forged policy, and a third feature with no render
holds the defaults with no mount, a value that is not a map, the two roots' nulling and the one-reader rule; each of
the last two was seen to fail with the nulling removed and with a direct read added);
`./moq test mr-boss --nobuild --spec InsightsAccess --spec InsightsDashboards --spec InsightsHome` (12 features);
`cargo test -p moqui-host --test it tools_screens -- --ignored` (Monitor's Logs, Metrics and Compare include the
changed screens; 27 s) and, of the same binary by name, `monitor_tui::`, `monitor_readers::`,
`report_embed_renders_the_dashboard_for_a_signed_in_viewer_only` and `report_documents::` (every changed Dashboards
screen in html or the terminal; 6 pass, 3 need a server that is not there); `moqui lint --screens` with no parse
failure and no flagged site that is new, `moqui lint` of `ContextServices.xml`. Changed and not run: mr-boss's
acceptance `InsightsTui` and `InsightsServerStatic` (the screens mounted by location added to their lists).

### I1 Datasets (Nov 11, 2026)

Built: the four dataset screens of Dashboards, Insights' with their BOSS references gone: `Datasets` (the business
datasets the viewer reads and a steward's waiting for review, with state, source, scope, index and last build; a new
dataset over a registered feed's document; below it, where there is no mount, "Other datasets", what the screen showed
before: the telemetry indexes, the document indexes and the saved datasets that are not business ones, with the fields
of the one chosen), `DatasetDetail` (identity and scope, admission checks, Admit, Rebuild index, Retire, the definitions
that use it; with no mount, the dataset in the definition editor), `DatasetFields` (the document's fields and
conditions) and `DatasetView` (the rows as the viewer reads them, a page at a time, and their export as CSV or JSON,
recorded). Their presentation is `tools/template/ReportDatasets.js` (`list`, `detail`, `view`, `exportRows`,
`createRequest`, `storeRequest`), which calls only `ReportServices` as the viewer and shows a refusal by its words.
Two operations in the new `framework/service/org/moqui/report/ReportReads.js`: `get#BusinessDataset` (one dataset the
caller reads or stewards, as `find#BusinessDatasets` answers it; for its steward its document's fields and conditions
and the definitions over it; any other dataset, and one that does not exist, "not available") and
`get#BusinessDatasetOptions` (the registered scope policies with their scope parameter and the engine's operators and
functions for any signed-in caller; `canCreate`, the dataset-save artifact by the caller's own grant, and only then the
documents of registered feeds; it takes nothing from the request). Both are covered by `DASHBOARDS_APP`'s
`ReportServices.*` pattern, as every other operation is, so the framework's security data gains no row; they are in
Monitoring.md ("Business datasets") and `JS-Framework-Services.md`. In the engine (`crates/moqui-report/src/business.rs`;
the release binary rebuilt): a scope policy's registration names its scope parameter (the enumeration's `optionValue`,
`groupField` for `ReportUserGroups`), answered as each dataset's `scopeParameter`; and `create#BusinessDataset` asks the
update artifact of the dataset named beside the dataset-save artifact. `ReportMount.js`'s policy gains
`datasetScopeLabel` and `datasetScopeHelp`. Datasets and Explore have a menu image, as the other Dashboards items now
do. In mr-boss: the four screens include the Dashboards screens at their own locations and take in the eight
transitions Insights offers (the table below); `Datasets.js` and `DatasetServices.xml` keep `check#DatasetReadiness`
and `refresh#DatasetIndex`, and the six services of the screens are deleted; the dataset prefix is written once, in
`Insights.js` `reportPolicy`, with Insights' words for the organization field; the two operations are rows of
`BOSS_INS_DATASET_READ`, beside `find#BusinessDatasets`.

| Screen | With no mount | Under a mount | Insights | Taken in | Left out | BOSS rows that name it |
|---|---|---|---|---|---|---|
| Datasets | the business list, New dataset, then Other datasets with their Explore and Definition links and the fields of one | the business list and New dataset; the id starts with the mount's prefix, the scope field has the mount's words | W | `createDataset` | `explore`, `definition` | `BOSS_INS_DATASET_SCREENS`: the screen, `Datasets.xml/createDataset` |
| DatasetDetail | the dataset for its steward, with Definition in the More menu | the same without Definition | W | `storeDataset`, `admitDataset`, `retireDataset`, `rebuildIndex` | `definition` | `BOSS_INS_DATASET_STEWARDSHIP`: the screen and the four transitions |
| DatasetFields | fields and conditions | the same | W | `storeFields`, `storeConditions` | none | `BOSS_INS_DATASET_STEWARDSHIP`: the screen and the two transitions |
| DatasetView | the rows and the export, the range Tools' | the same, the range the mount's | W | `exportDocuments` | none | `BOSS_INS_DATASET_SCREENS`: the screen, `DatasetView.xml/exportDocuments` |

Each grant is a non-inheritable always-allow on the screen and on its transitions by name, with the root group and
the administrator denied, so a wrapper is the only form that keeps them: no row is changed, and a transition Insights
does not take in would be refused even if it were on the route. `BOSS_INS_LEGACY`'s plain names match nothing, as
before.

Where what `Datasets.js` did is now:

| Insights service | Now |
|---|---|
| `find#Datasets` | `ReportDatasets.list` over `find#BusinessDatasets`; `canCreate` is the engine's (the dataset-save artifact), not `INSIGHTS_DATASET_EDIT` |
| `get#Dataset` | `get#BusinessDataset`: the document's fields and conditions read by the framework, for the dataset's steward |
| `get#DatasetChoices` | `get#BusinessDatasetOptions` |
| `create#Dataset` | `ReportDatasets.createRequest` (the mount's prefix and the form's short name), then the engine's `create#BusinessDataset` |
| `get#DatasetView` | `ReportDatasets.view` over `query#Documents` |
| `export#Dataset` | `ReportDatasets.exportRows` in DatasetView's `exportDocuments`, over `query#Documents` and `audit#Export` |
| `check#DatasetReadiness`, `refresh#DatasetIndex` | stay: Admin's registries name them |

No write operation was missing in the engine.

Findings:

- F53. F49 is settled so: the scope parameter's name is the policy's, from its registration, and the words of the
  form's field are the app's, from the mount. A dataset form has one scope field and sends it under the parameter of
  the policy chosen (`groupField` for the framework's policy), which keeps Tools right with no mount whichever policy
  is chosen; Insights' form sent `organizationField` under any policy. The mount's `datasetScopeParameter` is kept as
  the name for a policy whose registration names none, and `datasetScopeLabel` and `datasetScopeHelp` are Insights'
  "Organization field" and its help (Tools: "Scope field"). Mantle's policy is registered in mr-mantle, which this
  phase does not change, so it names no parameter yet: under Insights the mount's name serves it, and in Tools with no
  mount a dataset under Mantle's policy is asked for no scope field and its scope is shown without the field. One
  attribute there (`optionValue="organizationField"` on the `MantleOrganization` enumeration) completes it, and
  `datasetScopeParameter` can then leave the contract.
- F54. F50 for the prefix: the engine's `create#BusinessDataset` asked only the dataset-save artifact, so a steward
  granted the datasets of a name pattern could create one under any id by a direct call (a dataset they could then
  neither admit nor read; no data was reached). It now asks the update artifact of the dataset named, so the prefix a
  screen puts before a name gives no right. A scope field is taken by the engine only as a plain name, as before.
- F55. F45 is settled as far as the screens go. The dataset screens open Explore by a link of their own to the mount's
  `explore` URL, not by a transition, so an app takes in nothing and needs no grant row for it, and DatasetView's link
  sends `pageIndex` empty beside the dataset, the range and the query (Insights' spec fails without it). The Report
  builder's tab to Explore still carries the page: the subscreens menu builds each item's URL from what the item
  declares of the current request (from DatasetView with `pageIndex=2` the menu's data has Explore with `datasetId`,
  `from` and `pageIndex=2`, and Analysis with `datasetId` and `from`), in Tools' own menu too. Left as it is: the change
  would be the menu's, for every app.
- F56. Datasets' parameter for the other dataset whose fields are shown is `otherDatasetId`, not `datasetId`: the
  screens beside it carry `datasetId`, and a link or tab back to the list would have opened the other datasets' field
  list on a business dataset (F24).
- F57. The permission checks of the deleted services (`INSIGHTS_DATASETS_UNAVAILABLE` and the others) are not
  replaced: who opens a screen is its grant, what is offered is the engine's answer (`stewarded`, `canCreate`), and an
  author who posts `createDataset` is refused by the operation's own grant (held in `InsightsDatasets`). A reader's
  export is refused at the screen, where it was refused by the service.
- F58. Both rows are in `BOSS_INS_DATASET_READ`, not one in the steward's group: the list asks the options at every
  render, an author's too, and the operation answers the sources only to a caller who may create.
- F59. The report engine refuses in sentences, not coded reasons, so `AdminCalls.read` would show its fallback in
  place of them; `ReportDatasets.js` has its own `ask` (a transaction and an error frame of its own, the first line of
  the refusal). A service an operation of `ReportReads.js` calls as the caller needs the caller's own grant under
  Insights (a service row is not inheritable there): `find#BusinessDatasets` and `find#DefinitionUses`, which the
  reading groups hold.
- F60. mr-boss's legacy saved-find mapping found a list by reading the mr-boss screen file, and a wrapper has no
  form-list (`InsightsCatalog` failed): `findList` reads the framework screen a file includes too.
- F61. The export's default row limit is passed to its form as text: a number in a form's map is shown formatted
  ("1,000") in a number field. In html a `container-dialog` shows its button text as the dialog's title.
- F62. No recorded agent replay names a screen or a service of this phase (not replayed).

Tests: `cargo lint` and `./moq build` (the Rust change), then `./moq test tools --nobuild` (72 features, 3.3 to 3.4 s
of 4 s; `ReportMounts` holds the two new policy fields in the no-mount policy, no render added); `./moq test framework --nobuild --spec
BusinessDatasets` (8 features, 0.6 s; the new feature: what each read answers a steward, a reader and a creator, a
dataset the caller does not read told as one that does not exist, a dataset created under an id outside the creator's
grant and a scope field that is no name refused, nothing stored); `./moq test mr-boss --nobuild --spec InsightsDatasets
--spec InsightsAccess --spec InsightsHome --spec InsightsCatalog --spec InsightsSettings --spec InsightsDashboards
--spec AdminExports --spec AdminRegistries` (41 features; InsightsDatasets' one render is DatasetView through Insights'
route, with the Explore link and no page in it, the export through its transition, and a new feature with no render:
the steward's dataset created and revised through the route, an author's refused, and `Datasets/definition`,
`Datasets/explore` and `DatasetDetail/definition` posted by the steward and by ADMIN, not on the route, where Tools'
own route goes to its editor); `cargo test -p moqui-host --test it tools_screens -- --ignored` (the four screens in
html with no mount over a seeded business dataset: the other datasets, the scope by the registered parameter, the
definition link, and a dataset made there with no prefix and its scope field under `groupField`; 26 s) and `cargo test -p moqui-host --test it -- monitor_tui:: monitor_readers::` (the three new
screens in the terminal walk; 4 pass, 1 needs a server that is not there); `moqui lint --screens` with no parse
failure and no flagged site in the changed screens, `moqui lint` of the two changed service files. Changed and not
run: mr-boss's acceptance `InsightsDatasetsUsable` and `InsightsReviewWalk` (the export through DatasetView's
transition in place of the deleted service).

### I2 Delivery (Nov 11, 2026)

Built: the three delivery screens of Dashboards, Insights' with their BOSS references gone: `Schedules` (menu
"Scheduled reports": Mine, the schedules that go to the viewer alone with the owner-mode ones and the screen schedules
they set up for themselves; Shared administration, the group schedules they own; Received, the reports delivered to
them; New schedule, for oneself or for people and groups; the bulk pause; with no mount a link to Reports),
`ScheduledReportDetail` (the timing and output, the recipients beside each one's result of the engine's check, the
viewer's own preview, the runs, Resume, Send now and Pause; a screen schedule has its timing alone, an owner-mode one is
paused, resumed and sent and not changed) and `ReportDelivery` (one run: what ran and each recipient's outcome for its
owner, with retry and reconcile, and for the administrator with its owner by name; a recipient's own delivery and
their own report rendered again; the statuses as CSV). Their presentation is `tools/template/ReportDelivery.js`
(`list`, `detail`, `delivery`, `recipientChoices`, `create`, `storeTiming`, `storeAudience`, `pause`, `send`,
`previewPage`, `ownReportPage`, `retry`, `reconcile`, `auditCsv`), which calls only `ReportServices` as the viewer and
shows a refusal by its words. Three operations: `find#ReportRecipients` and `find#ScreenSchedules` in `ReportReads.js`
and `store#ScreenSchedule` in the new `ReportWrites.js` (below); they are covered by `DASHBOARDS_APP`'s
`ReportServices.*` pattern, so the framework's security data gains no row, and are in Monitoring.md ("Report delivery")
and `JS-Framework-Services.md`. In the engine (`crates/moqui-report/src/delivery.rs`; the release binary rebuilt):
`get#ReportOccurrence` answers `ownerName` with the whole run, to its owner and to the administrator.
`ReportMount.js` gains `policyWith(fields)`. `Reports.xml` keeps the previews, the composer's link, the playlists and
the owner-mode form (`create#Schedule`, then Schedules), and loses the schedule list with `send` and `pause`. Schedules
has menu index 8 and its icon; Annotations and Git sync move to 9 and 10. In mr-boss: the three screens under
`Insights/ScheduledReports/` are deleted and the area's items name the Dashboards files; the area's `always-actions`
add the recipient cap to the policy (`ContextServices.get#DeliveryPolicy`, then `policyWith`); `DeliveryServices` keeps
`find#RecipientChoices`, `preview#Report` and the four digest services, and `Delivery.js` what they and `Alerts.js`
use; `InsightsDeliveryTransitions.xml` keeps `recipientChoices` (AlertDetail) and `previewFile` (ManagementDigest);
the three operations are rows of `BOSS_INS_REPORT_OWN`, beside `store#Schedule` and `list#Schedules`.

| Screen | With no mount | Under a mount | Insights | Transitions | BOSS rows that name it |
|---|---|---|---|---|---|
| Schedules | every dashboard and report of dashboards the viewer reads; recipients by the engine's rule; the engine's template and cap; the link to Reports | without the hidden components' definitions; recipients among the holders of the mount's permission; the mount's template and the area's cap; no link to Reports | D | all on the route: `createSchedule`, `pauseSchedules`, `sendNow`, `recipientChoices`, `ownReportFile` | none |
| ScheduledReportDetail | the same rules for a change of recipients | the same under the mount's policy | D | all: `storeSchedule`, `storeAudience`, `pauseSchedule`, `sendNow`, `recipientChoices`, `previewFile` | none |
| ReportDelivery | the run for its owner, the administrator or a recipient | the same | D | all: `retryRecipients`, `reconcileRecipient`, `exportAudit`, `ownReportFile` | none |

No row of `BossSetupData.xml` names the three files or a transition of theirs: `BOSS_INS_LEGACY`'s rows name the area
file, as plain names that match nothing (P7), and the area is opened by the root's allow. Nothing is left out, since
nothing of the three is Tools' own; Tools' owner-mode form is Reports', which Insights does not mount.

Where what `Delivery.js` did is now:

| Insights service | Now |
|---|---|
| `find#Schedules` | `ReportDelivery.list` over `list#Schedules`, `find#ReportOccurrences`, `find#ScreenSchedules` and `find#ReportRecipients` (names, the viewer's groups, `canDeliver`) |
| `get#Schedule` | `ReportDelivery.detail`, with `check#ReportAudience` and `get#Definition` |
| `store#Schedule` | `ReportDelivery.create`, `storeTiming` and `storeAudience` over `store#Schedule`; a screen schedule over the new `store#ScreenSchedule` |
| `pause#Schedules`, `send#Schedule` | `ReportDelivery.pause` (`pause#Schedule`, or `store#ScreenSchedule` for a screen schedule) and `send` (`send#Report`) |
| `check#Audience` | the engine's `check#ReportAudience`, asked by `detail` |
| `find#RecipientChoices` | `find#ReportRecipients` for the schedule screens; the Insights service stays for AlertDetail until I3 |
| `get#Delivery`, `export#DeliveryAudit` | `ReportDelivery.delivery` and `auditCsv` over `get#ReportOccurrence` (with `ownerName`) and `audit#Export` |
| `retry#Recipients`, `reconcile#Recipient`, `get#OwnReport` | `ReportDelivery.retry`, `reconcile` and `ownReportPage` over the engine's three |
| `preview#Report`, the four digest services | stay |

The operations:

| Operation | Answers or stores | Checks |
|---|---|---|
| `find#ReportRecipients` | `canDeliver`; a page of active internal accounts by name (`users`); the caller's own groups (`groups`); of the `userIds` and `groupIds` named, `selectedUsers`, `refusedUserIds`, `refusedGroupIds`. With `permissionId` only its holders | signed in; the group delivery artifact by the caller's own grant, without which no account and no group is answered and every other name is refused; the caller is always accepted |
| `find#ScreenSchedules` | the caller's own screen schedules (the caller as `userId`, no report definition), with the saved find's description and `groupAdmitted` | signed in; only the caller's rows, whatever id is asked |
| `store#ScreenSchedule` | the timing, format, skipping when empty, interval, subject and pause of one of them; nothing else of the row | the row found again by its user; another's, a report schedule and none refused alike (`SCHEDULE_UNAVAILABLE`); a time pattern, a known format and a plain subject (`VALUE_INVALID`) |
| `get#ReportOccurrence` | `ownerName` in `occurrence` | as before: answered with the whole run, to its owner and to a holder of `ADMIN_TRANSFER_VIEW` |

The policy values, and why a hand-made request does not get past them:

| Value | Where it is enforced | Why it holds |
|---|---|---|
| `recipientPermissionId` | `ReportDelivery.create` and `storeAudience` ask `find#ReportRecipients` with it for the people and groups the request names and refuse the save when one is refused; `recipientChoices` proposes with it | the transitions read the policy the root set, never a parameter; under Insights they are the only way to `store#Schedule` with recipients: the operation is granted to the reading groups for these screens, no REST resource or agent tool names it, Insights' own `store#Schedule` service is deleted and its users open no Tools route. The engine then checks each recipient's scope, as before |
| `reportEmailTemplateId` | `create`, `storeTiming` and `storeAudience` put it on every `store#Schedule` request | no transition takes a template from the request; a posted `emailTemplateId` is not a parameter of theirs (held in `InsightsDelivery`) |
| `recipientCap` | the same three put it on every request; `store#Schedule` refuses a larger audience (`RECIPIENT_CAP`) and holds the cap under the instance ceiling, and a resume and each run check the stored cap | the area sets it from `get#DeliveryPolicy` through `policyWith` in `always-actions`, after the root, for every transition under it; a posted `recipientCap` or `reportPolicy` is ignored (held in `InsightsDelivery` and `InsightsSettings`, each seen to fail with the area's addition removed) |

Findings:

- F63. The three screens are mounted by location, as the plan had it. What Insights' services did beside the engine
  was permissions, the cap, the template, the readers and words, and each has a place: the grants, the area, the
  root's policy, `find#ReportRecipients`, and the screens' own sentences.
- F64. The `INSIGHTS_SCHEDULE` and `INSIGHTS_DISTRIBUTE` checks of `Delivery.js` are not replaced. What a viewer is
  offered is their own grant of each engine operation (`isPermittedOwn`, by the action the operation's verb asks), so
  a reader of deliveries is not refused a list of schedules at every render: reading deliveries is
  `BOSS_INS_REPORT_DELIVERY` (the five reading groups and the administrator), scheduling `BOSS_INS_REPORT_OWN` (the
  five), delivering to others the engine's group delivery artifact (`BOSS_INS_GROUP_DELIVERY`, the distributor). The
  setup data gives each permission and its grant to the same groups; a group given one without the other would now
  follow the grant.
- F65. F50 for the recipients: the engine takes any user and any group a caller with the group delivery artifact
  names, and judges scope. "Only the app's readers" and "only a group you belong to" were Insights' rules; they are
  `find#ReportRecipients`' now (the permission as its parameter, the caller's own groups always), held by the
  transitions for Tools too. The engine's `store#Schedule` itself is unchanged: Reports' owner-mode form and a direct
  call still name any group. A screen that saves an audience another way (I3's alert rule) must ask the operation
  first, as `ReportDelivery.unaccepted` does.
- F66. The operation is granted to every group that schedules, not to the distributor alone, as F58: the list asks it
  at each render for the names and for `canDeliver`, and it answers an account or a group only to a caller who holds
  the group delivery artifact. Insights' alert authors are in those groups already.
- F67. A transition's refusal is its error (the engine's sentence without its code and with a capital), where the
  Insights services answered `applied false` with a reason shown as a message. The specs and the acceptance scenario
  read the errors. No reason code reaches a page; nothing compared one.
- F68. An owner-mode schedule (made on Reports, or by `create#Schedule`) is listed under Mine as "Owner mode" and is
  paused, resumed and sent on the new screens, not changed: `store#Schedule` on it would make it a business schedule
  and drop its location and its group, which Insights' detail did when its form was saved. A change of one is still
  made nowhere, as before.
- F69. ReportDelivery shows the whole run to a holder of `ADMIN_TRANSFER_VIEW` who is not its owner, as
  `get#ReportOccurrence` answers it (the counters, each recipient's outcome, no title, the owner by name), read only.
  Insights' screen showed such a viewer the heading alone, though its service answered the same rows. Under Insights
  this reaches a user who holds both an Insights reading group or the administrator's and that permission.
- F70. The schedule list reads `reportHiddenComponents`, as the dashboard list does, and `createSchedule` takes only a
  definition the list offers: under Insights one of Tools' dashboards is not scheduled, by hand either, where the
  Insights service took any definition its caller read. A report definition is offered when it has dashboards (a
  playlist and a flow map have none), named with its kind ("digest").
- F71. Names for ids come from `find#ReportRecipients` (`selectedUsers`, the caller's groups): `list#Schedules` answers
  ids. A recipient who is no longer accepted shows as "1 person" in the list and as "A person who may no longer
  receive it here" among the recipients, with their name in the audience check beside it. The client sends a
  server-search drop-down's term and no selected value, so ScheduledReportDetail passes the schedule's recipients as
  `selected` in the options' `parameter-map`; not looked at in a browser.
- F72. mr-boss's legacy mapping moved a saved find of the copied scheduled list to Insights' own list, whose file is
  gone and which kept no saved finds; the entry is dropped, so such a find shows as not valid, as the other replaced
  lists' do. The schedule path mapping stays.
- F73. `get#DeliveryPolicy` is a service of Insights' own for the area's `always-actions`: the cap is the
  organization's setting, read once for each request under the area (a keystroke of the recipient choice included),
  where `Delivery.js` read it at each save and check.
- F74. For I3: AlertDetail still takes `recipientChoices` from `InsightsDeliveryTransitions.xml`
  (`find#RecipientChoices`), and `Alerts.js` uses `Delivery.js`'s `engine`, `attempt`, `proposable`, `userNames`,
  `groupNames`, `dateText` and `deliveryStates`; `proposable` reads the permission from `Insights.js`
  (`recipientPermissionId`, the one place). The Alerts area adds the cap as Scheduled reports does. `ownerName` for
  `get#AlertState` is `alert_delivery.rs`'s to add. For I4: Reports keeps `createPlaylist` and `playlist`, which
  Insights' Playlists takes in.
- F75. `rustfmt` on the two host test files reformatted lines that are not this phase's (six places); they are put
  back.
- F76. No recorded agent replay names a screen, a transition or a service of this phase (not replayed).

Tests: `cargo lint`, `./moq build`, then `./moq test tools --nobuild` (72 features; 3.3 s of 4 s in the last two runs
and 3.4 before them; the first run after the build measured 4.2 s and the three after it 3.5 to 3.9, the spread in one
DataViews feature, 1.4 to 1.7 s; `ReportMounts`' three features take 0.07 s: its third holds `policyWith` with and
without a mount, no render added);
`./moq test framework --nobuild --spec ReportAudience --spec BusinessDatasets` (20 features, 1.8 s; the new feature of
ReportAudience, 0.24 s: who is proposed and accepted by the engine's rule and under a permission, a disabled, an
external and an unknown account refused, a caller without the group delivery grant and one without the operation,
the cap, and a screen schedule read and changed by its user alone, with another's, a report schedule, a bad time
pattern, format and subject refused; `ownerName` is in the existing feature of the occurrence);
`./moq test mr-boss --nobuild --spec InsightsDelivery --spec InsightsSettings --spec InsightsAccess --spec
InsightsHome --spec InsightsCatalog --spec InsightsAlerts --spec InsightsDatasets --spec AppOrganizationFilters` (43
features; InsightsDelivery
posts the screens' transitions through Insights' route: a reader's own schedule and their refusal for others, a
distributor's readers accepted, a non-reader, a group that is not theirs, the Services reader and an audience past
the organization's cap refused, the template and the cap of a save whatever is posted, a Tools dashboard and Tools'
owner-mode form refused or not on the route, each taken on Tools' own route; its one render is ScheduledReportDetail's);
`cargo test -p moqui-report delivery` (8); `cargo test -p moqui-host --test it tools_screens -- --ignored` (the three
screens in html with no mount over a seeded schedule, an owner-mode one, a screen schedule and a run; a schedule of a
Tools dashboard made there; the status export; Reports as trimmed; 26 s) and `cargo test -p moqui-host --test it --
monitor_tui:: monitor_readers::` (the three screens in the terminal walk; 4 pass, 1 needs a server that is not there);
`moqui lint --screens` with no parse failure and no flagged site in the changed screens, `moqui lint` of the three
changed service files. Changed and not run: mr-boss's acceptance `InsightsDeliveryUsable` (a refusal read as the
transition's error; Send now and the saved list's recipients through the route in place of the deleted services) and
`InsightsReviewWalk` (the engine's `check#ReportAudience` in place of `check#Audience`).

### I3 Alerts (Nov 11, 2026)

Built: the alert screens of Dashboards, Tools' merged with Insights': `Alerts` is one list (All, every rule the viewer
reads, where rules have contact points; Mine; Received, the rules that told them with their own delivery; Templates,
the shipped rules, each copied over a dataset the viewer reads), each rule with its kind (an operational rule tells
contact points, a business rule an audience), its owner by name, its condition, whom it tells and the most severe
state of its series, with pause, activate (the rule's reading run as its owner first) and delete, one rule or several;
below, with no mount, the state of each series, Evaluate now and the shared contact points, silences and mute timings.
`AlertRule` is one editor: AlertBasis, the basis every rule has (name, query, measure and reduction, condition,
timing), then whom it tells by its kind: an operational rule's contact points, or a business rule's AlertAudience (the
owner alone, or people and groups beside each one's result of the engine's check, in the app and by account email, and
the group it is shared with) and AlertDeliveryStatus; either kind's silences, mute timings and shared items where rules
have them; and OwnAlertState, each series as the viewer may read it. `AlertShared` stays the shared item. Their
presentation is the new `tools/template/ReportAlerts.js` (`list`, `detail`, `store`, `storeAudience`, `pause`, `test`,
`copy`, `remove`, `recipientChoices`; Tools' own `evaluate`, `addContact`, `addSilence`, `addMute`, `removeItem`,
`addRef`, `removeRef`, `removeShared`; and `requireSet`, `basisValues`, `evaluationMessages`), which calls only
`ReportServices` as the viewer and shows a refusal by its words; it shares the engine call, the recipients and the
delivery words with `ReportDelivery.js`, which exports them. A rule's kind, state and owner's name are
`get#AlertState`'s: the direct read of `AlertInstance` in the old list is gone, and no screen reads an entity. The
three screens link each other by the mount's `alerts`, `alertRule` and `alertShared`, as links and transition answers
through `ReportMount.js`. In the engine (`crates/moqui-report/src/alert_delivery.rs`; the release binary rebuilt):
`get#AlertState` answers `ownerName`. No operation is added: the framework's security data, mr-mantle and the BOSS
artifact rows are unchanged. Dashboard offers a panel's New alert rule only where the policy's `features.alert` says
so. `datasetScopeParameter` left the contract (`ReportMount.js`, `ReportDatasets.js`, Monitoring.md, Insights' policy,
`ReportMounts`, `InsightsAccess`): mr-mantle's organization policy registers its parameter, and nothing else used the
fallback. In mr-boss: `Alerts/Rules.xml` and `Alerts/AlertDetail.xml` include the Dashboards screens at their own
locations and take in the ten transitions Insights offers (the table below); the Alerts area's `always-actions` add the
recipient cap to the policy, as Scheduled reports does; `Alerts.js` and `AlertServices.xml` are deleted;
`DeliveryServices` loses `find#RecipientChoices` and `Delivery.js` what only the alerts used (the recipient reads, the
names, the delivery words and its exports), keeping the digests and the preview; `InsightsDeliveryTransitions.xml`
keeps `previewFile` (ManagementDigest); `reportLinks.alertRule` is AlertDetail for every reader; the alert topic,
template and link are written once, in `Insights.js` `reportPolicy` (F48 is closed: the dataset prefix, the two
templates, the topic and the link each have one place).

| Screen | With no mount | Under a mount that names `alertAudience` | Insights | Taken in | Left out | BOSS rows that name it |
|---|---|---|---|---|---|---|
| Alerts | All (the default), Mine, Received, Templates; every rule the viewer reads with its kind and owner; New alert rule; Evaluate now; the series' states; the shared contact points, silences and mute timings | Mine, Received and Templates for one who may set a rule, Received alone otherwise; business rules only, without the hidden components'; a copy has the mount's fixed parts and waits to be activated; no New (a rule starts from a panel's menu), no series box, no shared items | W (`Alerts/Rules`) | `copyAlert`, `pauseAlerts`, `deleteAlert` | `evaluate`, `deleteShared` | none |
| AlertRule | the basis with the query typed or built, a rule ID, the evaluation state, series and routing labels and a version note; contact points for an operational rule, the audience for a business one; silences, mute timings and shared items; Evaluate now; the state; a new rule from nothing or from a panel's queries | the basis over a panel's saved query (measure of the query, range and interval from lists); the audience with the mount's topic, template, link and cap; the state and the deliveries; a new rule only from a panel, saved inactive; no contact point, silence, mute timing, shared item or evaluation | W (`Alerts/AlertDetail`) | `recipientChoices`, `storeAlert`, `storeAudience`, `pauseAlert`, `testAlert`, `copyAlert`, `deleteAlert` | `addContact`, `addSilence`, `addMute`, `removeItem`, `addRef`, `removeRef`, `evaluate` | none (the topic's `linkTemplate` and the policy's `detailLink` name its path) |
| AlertShared | as before, its links the mount's and its writes held to `features.alert` | not shown | not mounted (`alertShared` null) | | all | none |

No row of `BossSetupData.xml` names the two Insights files or a transition of theirs (the root's allow opens them), so
by the rows alone they could be mounted by location. They are wrappers because Tools' screens have transitions
Insights must not offer: a location mount would put the contact point, silence, mute timing, shared item and
evaluation transitions on Insights' route. Under Insights' policy each of them refuses (`change` refuses under a mount
of audiences, `evaluate#Alerts` is denied by `BOSS_INS_REPORT_TOOLS`), but the rule is that they are not there.

Where what `Alerts.js` did is now:

| Insights service | Now |
|---|---|
| `find#Alerts` | `ReportAlerts.list` over `list#Definitions`, `get#AlertState`, `find#AlertDeliveries`, `get#Dataset` and `find#ReportRecipients` (names) |
| `get#Alert` | `ReportAlerts.detail`, with `check#ReportAudience`; the owner's name from `get#AlertState` |
| `store#Alert` | `ReportAlerts.store` (the basis) and `storeAudience` over `save#Definition`; the fixed parts written by `fixedParts` |
| `copy#Alert`, `pause#Alerts`, `test#Alert`, `delete#Alert` | `ReportAlerts.copy`, `pause`, `test` (`run#Query`) and `remove` |
| `Delivery.js` `proposable`, `find#RecipientChoices` | `find#ReportRecipients` under the policy's permission (`ReportDelivery.js` `recipients`, `recipientChoices`) |
| `Delivery.js` `userNames`, `groupNames` | `find#ReportRecipients`' `selectedUsers` and the caller's groups; the owner by `get#AlertState` |

The policy values, and why a hand-made request does not get past them:

| Value | Where it is enforced | Why it holds |
|---|---|---|
| `features.alert` | `ReportAlerts.requireSet`, first in every transition that sets a rule: `storeAlert`, `storeAudience`, `pauseAlert` and `pauseAlerts`, `testAlert`, `copyAlert`, `deleteAlert`, Tools' own (`evaluate`, the contact point, silence, mute timing and shared item transitions, `deleteShared`) and AlertShared's save, copy and delete | the transitions read the policy the root set in `always-actions` (the area's `policyWith` keeps the root's fields), never a parameter; a posted `reportPolicy` is overwritten before the transition runs. The engine alone would not hold it: Insights' dataset steward is granted `save#Definition` for datasets and panels and is no alert author, and the engine cannot tell. Under Insights these transitions are the only way to save an alert rule: the two editors take only dashboards, reports and panels (`reportDefinitionTypes`), the other Insights services that save write digests, shares, panels and flow maps, no REST resource or agent tool names the operation, and its users open no Tools route (held in `InsightsAlerts`: the steward's post refused and nothing saved, the author's same post saved; seen to fail with the steward let past) |
| `alertAudience` (topic, template, link) | `fixedParts`, on every save under such a mount (`store`, `storeAudience`, `pause`, `copy`): the topic, the template where email is a channel and the link are written from the policy, and the rule's contact points dropped | no transition takes a topic, a template, a link, a query or a contact point from the request under such a mount: a new rule's query is the saved query of a panel the caller reads, a change keeps the rule's own, and a posted `topic`, `detailLink` or `queryJson` is no parameter that is read (held in `InsightsAlerts`). The engine checks that the topic and template exist and that the link is a plain server path, and refuses a contact point on a business rule |
| `recipientPermissionId` | `storeAudience` asks `find#ReportRecipients` with it for the people and groups named and for the group the rule is shared with, and refuses the save when one is refused; `recipientChoices` proposes with it | as for a schedule (I2): the transition reads the policy, it is the only way to an audience under Insights, and the engine then checks each recipient's scope for an active rule and again before each notification |
| `recipientCap` | `fixedParts` writes it into the audience at every save; `save#Definition` refuses a larger audience of an active rule (`RECIPIENT_CAP`) under the instance ceiling, and each notification checks the stored cap | the Alerts area sets it from `get#DeliveryPolicy` through `policyWith` in `always-actions`, for every transition under it; a posted `recipientCap` or `reportPolicy` is ignored (held in `InsightsAlerts`: an audience past the organization's cap refused whatever is posted, and the stored cap; seen to fail with the area's addition removed) |

Findings:

- F77. `alertRule` had two meanings: where a rule opens, and whether a panel offers New alert rule (Insights gave it
  only to one who may set a rule). The merged list links a rule for every reader (Received), so the link is the screen
  and the feature is the offer: Insights names AlertDetail for everyone, and Dashboard sends `appLinks.alertRule` only
  where `features.alert` is true. The test mount now names an alert rule screen with the feature off, so
  `ReportMounts`' existing dashboard render holds the rule (`links.alertRule` set, `appLinks.alertRule` null).
- F78. What decides the editor's form is the mount's `alertAudience`, not the rule: a mount that names it tells every
  rule to an audience, so a rule there watches a panel's saved query (Insights' rule: never a typed query), every
  save makes it a business rule, and only business rules are listed or opened there. With no `alertAudience` the query
  is typed or built as Tools' was, and the engine's answer for the saved rule (`business`) decides whether the page
  shows contact points or the audience. "Only business rules" replaces Insights' own test (Tools' component, or a
  dataset named `otel_`): Admin's failing job runs rule, shipped by mr-boss over the spans, is left out as before.
- F79. `features.alert` is the one policy value that stands for a permission the engine cannot check: with it false
  and the engine's save granted (the steward), only the transition refuses. Every other value narrows what a caller
  who may already set a rule can put in it, and the engine checks the result.
- F80. No operation was missing. `ownerName` is answered to anyone who reads the rule, with or without its state: the
  rule's definition already answers them the owner's id. A shipped rule has none.
- F81. The group a rule is shared with is accepted through `find#ReportRecipients` too (the caller's own groups, F50,
  F65), so it needs the group delivery grant: Insights' form offered it only to a distributor, and its service took
  an author's own group by hand. An author's audience save shares with no group, as the form did.
- F82. Tools' save of a rule dropped its share group (the old transition sent none, and the engine stores what it is
  given); the basis save keeps it now. Tools gains Pause and Activate as actions, beside the form's Evaluation field
  where the query is typed; an activation by the action runs the reading first, in Tools too.
- F83. With no mount the list opens on All, so Tools shows what it showed (every rule read, the shipped ones among
  them), and the series' states are a box under the list from the same `get#AlertState` answers, one call a rule
  listed. Templates' Copy is the engine's `copy#Definition` there; under a mount of audiences it is a read and a
  save, since the copy takes the mount's fixed parts and Insights' distributor holds the save, not the copy.
- F84. With no mount an audience's email channel is offered only when the rule already names a template: the engine
  has no default alert template, and the audience form takes no template id.
- F85. The recipient cap is the policy's at every save. `Alerts.js` kept the lower of the stored and the current cap
  on a save that did not change the audience; the two are equal whenever the area supplies the cap.
- F86. A single rule's refused pause or activation is the transition's error (F67); of several, each refusal is a
  message and the others go on, as the schedules' bulk pause.
- F87. The `INSIGHTS_ALERT_REQUIRED`, `ALERT_UNAVAILABLE` and other reason codes of the deleted services are gone
  with them; nothing compared one but the specs. A member of Insights' root group alone is told "No alert has told
  you anything yet." where the list said alerts are shown to Insights readers.
- F88. The screen lint reads a name that ends in Set as a collection (F26's rule for Rows): the list's flag is
  `settable`. A link whose `url` is an expression takes its id from that text, so these links have ids of their own.
- F89. Under a spec's Insights root a transition's answer by the mount's links (`//Insights/...`) does not resolve
  (F4), so `InsightsAlerts` posts under the server's root, where a copy opens AlertDetail and a delete the list. What
  the screens show a user is checked with no render: the module called with the links and policy Insights' context
  answers that user.
- F90. `AdminOverview`'s two source-reading features follow the module: the evaluation words are
  `evaluationMessages`, and AlertRule's number fields `basisValues`; AlertShared keeps its script in the screen, so
  its two fields are read as before.
- F91. `rustfmt` on the two host test files reformatted lines that are not this phase's (six places); they are put
  back.
- F92. No recorded agent replay names a screen, a transition or a service of this phase (not replayed).
- F93. For I4: `features.share` and `shareGroups` are still read by no screen; Dashboard's sharing should hold the
  feature in its transition as `requireSet` does, since Insights' author check is `share#Dashboard`'s today. The
  engine's notification of a rule with no `detailLink` opens the list with `definitionId`, which the list does not
  read (it never did): a link to AlertRule would open the rule. `ReportDelivery.js` and `ReportAlerts.js` each have their own
  words for a recipient who is not accepted; `ReportDatasets.js` still has its own engine call.

Tests: `cargo lint`, `./moq build`, then `./moq test tools --nobuild` (72 features, 3.4 s of 4 s in each of three
runs; the one Alerts render, `AdminOverview`'s, takes 0.05 s with the kinds and states read through the engine, and
no render is added; `ReportMounts`' three features hold the policy without `datasetScopeParameter` and the alert rule
link beside the feature); `./moq test framework --nobuild --spec BusinessAlerts --spec ReportAudience --spec
BusinessDatasets` (24 features, 2.3 to 2.5 s; `ownerName` is in BusinessAlerts' existing feature of the state: for a
recipient, for a reader who gets no state, and none for a shipped rule); `./moq test mr-boss --nobuild --spec
InsightsAlerts --spec InsightsDelivery --spec InsightsDatasets --spec InsightsAccess --spec InsightsHome --spec
InsightsCatalog --spec InsightsSettings --spec InsightsDashboards` (39 features, 11.5 s; InsightsAlerts posts the
screens' transitions through Insights' route: an author's rule over a panel saved, the same post refused to a reader
and to the steward, a measure that is not the query's refused, a distributor's audience accepted and then refused for
the Services reader, a group that is not theirs, a person who is no Insights reader and an audience past the
organization's cap, the topic, link and cap of a save whatever is posted, nine Tools transitions not on the route for
the distributor and for ADMIN and a rule with a typed query and a contact point taken on Tools' own; its one render is
AlertDetail's, which shows none of Tools' parts; each of the feature, the cap and the recipients' acceptance was seen
to fail with its enforcement removed); `cargo test -p moqui-report alert` (14); `cargo test -p moqui-host --test it
tools_screens -- --ignored` (the three screens in html with no mount over a seeded operational rule with a series'
state, a rule over the business dataset and a shared contact point; the templates; a new rule, and one made there
with a typed query and a contact point; 27 to 32 s) and `cargo test -p moqui-host --test it -- monitor_tui::
monitor_readers::` (the list and the editor in the terminal walk; 4 pass, 1 needs a server that is not there);
`moqui lint --screens` with no parse failure and no flagged site in the changed screens (135 flagged sites in all,
143 before the list's flag was renamed), `moqui lint` of the three changed service files. Changed and not run:
mr-boss's acceptance `InsightsAlertsUsable` (a refusal read as the transition's error; the rule deleted through the
route) and `InsightsReviewWalk` (the copy, the audience and the activation through the screens' transitions in place
of the deleted services).

### I4 Drill, Analysis, share, playlists, wall (Nov 11, 2026)

Built: sharing on `Dashboard` (the `share` transition and the ShareDashboard dialog: the owner of a saved dashboard
shares it with one of their own groups or one the mount offers, or makes it private), and four screens of Dashboards,
Insights' with their BOSS references gone: `Playlists` (the playlists the viewer reads with their dashboards, seconds
and sharing; New playlist; Share a playlist; Play full screen), `WallDisplay` (one playlist on a bare page of its own,
unattended, with its one transition `data`), `Drill` (from a panel's value to the panel's buckets, a breakdown by one
more dimension and a page of its records) and `Analysis` (an aggregate over a dataset the viewer reads from typed
choices, run as the viewer and saved as a library panel). `Playlist` stays the player in the app. Their presentation
is `tools/template/ReportShare.js` (`state`, `listState`, `share`, `requireShare`), `ReportPlaylists.js` (`list`,
`create`, `player`, `playerData`, `wall`, `wallData`), `ReportDrill.js` (`view`) and `ReportAnalysis.js` (`view`,
`save`, and what an app builds on: `fieldsOf`, `choicesOf`, `queryOf`, `resultOf`, `refusedOf`, `panelId`), each
calling only `ReportServices` as the viewer. In the engine (`crates/moqui-report/src/definitions.rs`; the release binary
rebuilt): `save#Definition` takes a group only from a saver who may name it. One operation, `find#ShareGroups`
(`ReportReads.js`); ADMIN's grant of the share artifacts (`REPORT_SHARE_ADMIN`, `MonitoringSecurityData.xml`).
`ReportMount.js`: the link keys `playlists`, `wallDisplay` and `analysis`, and Tools' own `drill`, so Tools' dashboards
have Drill in each panel's menu. `Reports.xml` gives up `createPlaylist`, `playlist` and its playlist box, and links
Playlists. The Dashboards menu: Explore 2, Analysis 3, Playlists 9, Annotations 10, Git sync 11. In mr-boss: Dashboard
takes in `share` and loses its own dialog, `shareDashboard` and its range defaults; WallDisplay includes the Dashboards
screen at its own location with `data`; Playlists, Drill and ReportBuilder › Analysis are Insights' own screens around
the Dashboards screens' sections; `DashboardServices.xml` is deleted; `Drill.js` and `Analysis.js` keep Insights' own
parts over the framework's modules; PanelDetail's repeated "Used in" box is dropped (the editor's own list shows it);
`find#ShareGroups` is a row of `BOSS_INS_REPORT_AUTHOR` and `BOSS_INS_SHARE_DISPLAY` gives the author the wall
displays' share artifact.

| Screen | With no mount | Under a mount | Insights | Taken in | Left out | BOSS rows that name it |
|---|---|---|---|---|---|---|
| Dashboard | Share for the owner: their own groups | Share where `features.share` holds: own groups and the mount's `shareGroups` the viewer may name | W | `share`, with the six it had | none | none (the root's allow opens it) |
| Playlists | the list, New playlist, Share a playlist, Play full screen, Edit | the same without the hidden components' definitions; New where the mount has an editor; sharing by the feature | S (`Playlists`): the wall displays' share links, the devices, Register as display | `createPlaylist`, `play`, `edit`, `sharePlaylist`; `data` of Playlist | none | none |
| Playlist | one playlist in the app over the request's range or Tools' | the same over the mount's range | included by Playlists when one is chosen | `data` | none | none |
| WallDisplay | one playlist full screen over Tools' range | the same over the mount's range | W | `data` | none | `BOSS_DEV_INS_WALL` (the route group's allow-list) and `BOSS_INS_WALL_DISPLAY` (the display accounts' allow): the screen and `WallDisplay.xml/data` |
| Drill | the page, with Tools' links | the mount's range and fixed variables | S: its header, DrillMeaning, the organization in the Breakdown form, record links | none (it has none) | none | none |
| Analysis | every dataset the viewer reads; Save for one who may save | the mount's range and calendar | S (`ReportBuilder/Analysis`): its own page and `saveAnalysis` | none | `saveAnalysis` (Insights' own of that name is on the route) | `BOSS_INS_ANALYSIS`: the screen and `Analysis.xml/saveAnalysis` |

Each is the kind the plan expected. WallDisplay is a wrapper for its rows (the standalone page at the route group's
`homePath`), Dashboard for what it sets and puts around the screen (`reportFixedVariables`, the organization, the
measures' meaning). Playlists, Drill and Analysis could not
be wrappers: each adds columns, a section or a form field inside the Tools screen's own boxes, and Analysis' grant
names a transition that must stay Insights' own.

The operations:

| Operation | Answers or stores | Checks |
|---|---|---|
| `save#Definition` (Rust) | as before, with the group it is given | the group is one the saver belongs to by a membership of their own (not `ALL_USERS`), or one whose share artifact, `org.moqui.report.share.<userGroupId>` (update), the saver's own grant holds; `SHARE_GROUP_UNAVAILABLE` otherwise; a group kept as stored is not asked again. Every save goes through it: copy, restore, import, pull, a business dataset's |
| `find#ShareGroups` (new) | `groups` (`userGroupId`, `description`, `own`): the caller's own groups and, of `groupIds`, those the caller may name by the artifact; `refusedGroupIds` | signed in; names only a group that is the caller's own or theirs to name. Covered by `DASHBOARDS_APP`'s pattern; under Insights a row of `BOSS_INS_REPORT_AUTHOR` (the author and the steward), beside `save#Definition` |

Where what Insights' services did is now:

| Insights | Now |
|---|---|
| `DashboardServices.share#Dashboard` | `ReportShare.share` in Dashboard's `share` and Playlists' `sharePlaylist`; its group rule is `save#Definition`'s, its author check the policy's `features.share` |
| `DashboardServices.get#ShareGroups` | `find#ShareGroups` for the two share dialogs; `ContextServices.get#ShareGroups` (the same function of `Insights.js`, now without the wall displays) for SavedReports and Analysis, with FlowMaps' and the digest's use of it |
| `Insights.js` `shareDashboard` | deleted; `shareGroups` and `shareGroupIds` stay for Insights' own forms |
| `Drill.js` | keeps the reader check, the scope, the default range, the cost filter, the meaning and the record screens; the engine call, the buckets, the breakdown, the rows and the links' parameters are `ReportDrill.js` `view` |
| `Analysis.js` | keeps the author check, the admitted datasets, the organization's calendar, the narrowing for Assist, its reason codes and `store`; the choices' lists, the fields, the query and the result's rows are `ReportAnalysis.js` |
| Playlists' list, devices and display share | the list is `ReportPlaylists.list`; the direct read of `UserGroup` is gone (a group's name is `find#ShareGroups`'); the devices stay |
| WallDisplay's page and `data` | `WallDisplay.xml` of Dashboards and `ReportPlaylists.wall`, `wallData` |

The policy values, and why a hand-made request does not get past them:

| Value | Where it is enforced | Why it holds |
|---|---|---|
| `features.share` | `ReportShare.requireShare`, first in `share` and `sharePlaylist`; a group named in Tools' `saveAnalysis` | the transitions read the policy the root set in `always-actions`, never a parameter. The engine alone would not hold it: Insights' steward and distributor hold `save#Definition` and are no authors. Under Insights these transitions are the only way to change a dashboard's or a playlist's group: the editors keep the group as stored (the `share` link), Insights' own saves name only the caller's own groups, no REST resource or agent tool names the operation, and its users open no Tools route (held in `InsightsDashboards`: the distributor's share of their own dashboard refused and the author's same request taken; seen to fail with the feature not asked) |
| `shareGroups` | the transition takes only a group it offers (the viewer's own, and the policy's that `find#ShareGroups` answers them); `save#Definition` then asks the group's membership or share artifact itself | the list is what is offered and gives no right. A group in the policy is taken only from one whose own grant holds its artifact (Insights: the author, `BOSS_INS_SHARE_DISPLAY`), by a direct call too; a group outside both is refused by the transition and by the engine (held in `InsightsWallDisplay`: the author's share with the wall displays taken, a distributor's save with that group refused by the engine; in `InsightsDashboards`: another group refused by the transition and by the engine's save; in `ReportMounts`: the mount's group not taken from a viewer without its artifact) |
| `range` | Playlist and Drill open on it when the request names none; WallDisplay's page and `data` use it always | `wallData` reads no `from` or `thru`; the device's request gets the range from the root (held in `InsightsWallDisplay`: thirty years asked, thirty days answered; in the cargo test for Tools) |
| `calendar` | Tools' Analysis writes it into a time grain's bucket in its actions and in `saveAnalysis` | no parameter names a zone or an offset. Insights' Analysis is its own screen and service and writes the calendar of its own organization choice (`calendarParameters`), as before, so nothing on Insights' route reads the root's field today |

What keeps the wall display hardened: the route group and the display account's grant still name
`mr-boss/screen/Insights/WallDisplay.xml` and its `data` alone, and the wrapper takes in nothing else (the Tools screen
has nothing else); the page is standalone and bare, not the shell; `data` takes the playlist's index only, renders only
a dashboard of the stored playlist as the viewer, over the mount's range, drops each panel's inspection and requests,
shows a failed panel as "not available" with its text logged, and answers one fixed sentence for any failure; the
client's kiosk player is unchanged; Insights' root still calls no Insights service for a device and sets only the range.
A device opens neither Tools' `WallDisplay` nor its `data` (held in `InsightsWallDisplay`).

Findings:

- F94. F50 is settled in the engine with an artifact a group, not a list passed with the request: a list of allowed
  groups in the request would be the caller's word, and a single "may share with any group" grant would let an app's
  author name every group. `org.moqui.report.share.<userGroupId>` by the saver's own grant says exactly which group,
  is asked by `permitted_own` (nothing inherited from the screen's allow, no failure recorded) and is granted as the
  group delivery artifact is. `ALL_USERS` is not a group of one's own for this. A group kept as stored is not asked
  again: an owner who left a group, or a mounted editor's save, keeps it.
- F95. `find#ReportRecipients` could not name the groups: it answers them only to a holder of the group delivery
  artifact, and an author is no distributor. `find#ShareGroups` also names a definition's present group for the list,
  only when it is the caller's own or theirs to name; a reader sees "Shared".
- F96. Tools offers the dialog with no mount too, since a share with a group of one's own is the common case and the
  editor's group field is text. The link key `share` stays null for Tools: it names the screen a mount shares in and
  is what makes the editor keep a definition's group, which Tools' editor must not do.
- F97. Insights' `sharePlaylist` took only the wall displays or nothing; the framework's takes any group it offers,
  the sharer's own among them. Insights' own forms (a saved report's copy, an analysis, a flow map, a digest) offered
  the wall displays and then refused them; `shareGroups` of `Insights.js` is the user's own groups again.
- F98. The reason codes of `share#Dashboard` (`INSIGHTS_AUTHOR_REQUIRED`, `DASHBOARD_NOT_OWNED`,
  `DASHBOARD_REQUIRED`) are gone with it; a refusal is the transition's error (F67). `SHARE_GROUP_UNAVAILABLE` is the
  engine's now.
- F99. Three playlist screens, each one thing: Playlists the list and what is done to a playlist, Playlist one played
  where someone sits (the request's range, links out), WallDisplay one played where nobody does. The two `data`
  transitions share `dashboardAt` and differ in what they take and show. Playlist's three `now-7d` defaults are the
  policy's range. `createPlaylist` refuses a dashboard of a hidden component, as `createSchedule` does (F70).
- F100. New playlist is offered by the viewer's own grant of the save where the mount has an editor, as Insights
  offered it; the transition itself is held by the engine's save alone, as before, so Insights' distributor could
  post one by hand. There is no feature for authoring in the policy; not added here.
- F101. A service's context is its own: `ReportMount.js` called from Insights' drill service sees no mount and would
  answer Tools' range, so `view` takes the range and the fixed variables as options, and an app's service passes them.
  The same holds for any module function an app calls from a service.
- F102. `section-include` reaches a section nested in another, and a form extended from the Tools screen keeps its
  layout with an added hidden field: Insights' Breakdown form carries the organization that way. A row of the records
  list links what the app gives it (`recordUrl`, `recordParameters`), so the list itself is included.
- F103. The generic Analysis offers every dataset the viewer reads (`list#Datasets`), a telemetry index among them;
  Insights offers the admitted business datasets, from its own service. Its Save form no longer carries
  `organizationPartyId`: the Source form never did, so a run and its save both use the organization in scope.
- F104. Tools' `saveAnalysis` reads the dashboard to place the panel on before it saves the panel; a placement the
  engine then refuses (a stale version) leaves the panel saved and says so. Insights' `store#Analysis` is one
  transaction, as before.
- F105. In html a link to a transition that only redirects is written as its target (`Playlist?definitionId=`), which
  the cargo test holds.
- F106. `rustfmt` on the two host test files reformatted lines that are not this phase's; they are left as they
  were. `InsightsRecords` and `InsightsWallDisplay` were not prettier-clean before this phase and are not reformatted.
- F107. No recorded agent replay names a screen, a transition or a service of this phase (not replayed).
- F108. Left for the finishing phase: `B/plan` (design-insights §4 ShareDashboard and Playlists, impl-insights' P2,
  P6 and P11 entries name `DashboardServices`); the root's `calendar` field has no reader on Insights' route (the
  table above); `InsightsSpecLib.mountedScreens` lists only screens mounted by location, which this phase did not add
  to.

Tests: `cargo lint`, `./moq build`, then `./moq test tools --nobuild` (73 features; 3.4 to 3.5 s of 4 s in
four of five runs after the last build and 3.8 in one, the spread in the one DataViews feature, 1.4 to 1.7 s, as in
I2; `ReportMounts`' four features take 0.07 s, the fourth with no render, 0.01 s: the share transition under the test mount with the
mount's group, another group and the viewer's own, and the feature off; and the new link keys); `./moq test framework
--nobuild --spec ReportAudience --spec BusinessAlerts --spec BusinessDatasets --spec ReportDrill --spec
ReportAnnotations` (27 features, 2.6 s; the new feature of ReportAudience, 0.18 s: a group of the saver's own taken,
one they are not in, one no person is in, `ALL_USERS` and one that does not exist refused, a kept group saved and
moved, the wall group taken from a holder of its artifact and another group not, and what `find#ShareGroups` answers
each); `./moq test mr-boss --nobuild --spec InsightsRecords --spec InsightsDashboards --spec InsightsWallDisplay --spec
InsightsAlerts --spec InsightsDelivery --spec InsightsDatasets --spec InsightsAccess --spec InsightsHome --spec
InsightsCatalog --spec InsightsSettings --spec AdminDevices --spec InsightsFlowMaps` (56 features, 14.1 s; the share
through Insights' route in InsightsDashboards and InsightsWallDisplay, each seen to fail with the feature not asked; the distributor's `saveAnalysis` refused and the author's taken
in InsightsRecords; no render added); `cargo test -p moqui-report definitions` (2); `cargo test -p moqui-host --test it
tools_screens -- --ignored` (a playlist made, shared and played, the wall display's page and its data over Tools' range
whatever is asked, an index outside the playlist, a dashboard shared, an analysis run and saved onto it, and its drill;
29 s) and `cargo test -p moqui-host --test it -- monitor_tui:: monitor_readers::` (the four new screens in the
terminal walk; 4 pass, 1 needs a server that is not there); `moqui lint --screens` with no parse failure and no
flagged site that is new (135, as before), `moqui lint` of the four changed service files. Changed and not run:
mr-boss's acceptance `InsightsWallDisplayUsable` (the share through Dashboard's and Playlists' transitions) and
`InsightsReviewWalk` (the wall's definitions saved with their group in place of the deleted service).

### A6 Registries (Nov 11, 2026)

Built: the three registries of IronStone Admin are the framework's. Entities `moqui.server.ReadinessProvider`,
`RefreshOperation` and `SettingsEntry` (`framework/entity/ServerEntities.xml`), Admin's with their BOSS paths gone: a
provider's `fixPath` is `fixRoute` and `fixId`, a settings entry's `screenPath` is `destinationRoute` (the key of its
place) and `screenLocation` (the screen whose authorization decides who is shown it), and both a provider and a refresh
name the `scopeParameters` their service takes; `scopeEnumId` stays, for display. `org.moqui.admin.ReadinessServices`
(`Readiness.js`): `get#Readiness`, `request#Refresh`, `get#Overview`, `get#SupportSummary`, `find#Settings` and
`check#InstanceSetup` (the table below), granted to `ALL_USERS` in `AdminReadinessServices`
(`AdminConfigurationData.xml`) by an ALLOW that is not inheritable, and described in `docs/dev/JS-Framework-Services.md`,
`docs/security/Attack-Surface.md` and, for a component's author, `docs/dev/Practices-App-Dev.md` ("Readiness, refresh
and settings registrations"). No permission is added: the overview's is `ADMIN_OVERVIEW_VIEW`, as it was. The
framework seeds the registries' kinds (`AdminHealthGroup`, `AdminRefreshSource`, the Instance scope, the Applications
and Instance settings groups), ten providers of its own, each a function behind `check#InstanceSetup` (Admin's eight
checks, the protocols and the instance's Constitution, whose two adapters are functions there now), and seven settings
entries for its System screens (instance settings, localization, connections, mail relays, mail and protocols, message
types, notification topics). Screens: `System/Operations/Health.xml` (the checks by group, state and owner; Check
again; Request refresh with the check's own scope; a fix place linked by the mount's route), `SupportSummary.xml` (the
summary, its print and `summaryJson`, always redacted) and `System/Settings/SettingsHome.xml` (the entries that have a
place under the mount), with `template/AdminReadiness.js` (`health`, `settingsIndex`, `supportSummary`, `scopeOf`,
`linkFields`); menu items Health (Operations, index 4) and Settings index (Settings, index 3), and both in the home's
catalog. None has a part that is the instance administrator's alone: no old System screen did what they do.
`AdminMount.js`'s framework routes gain `adminHealth`, `supportSummary`, `adminJobRuns`, `adminTopics`,
`adminSystemMessages`, `adminMessageTypes`, `adminConnectionDetail`, `adminProtocols`, `adminUsers`, `adminVisits`,
`monitor`, `monitorSources`, `constitution` and `emailTemplates`, and `systemPlace(key, id, more)`, a framework route as
System's own absolute place. `get#SystemAttention` lists the instance's readiness checks that are not ready beside its
own readings and counts (F115). In mr-boss: the three entities are gone from `AdminEntities.xml`; the 64 seed rows of
`BossSetupData.xml` (23 providers, 8 refreshes, 33 settings entries) are 54 rows of the framework's entities (13
providers, 8 refreshes, 33 settings entries, 7 of them the framework's own keys seeded again with Admin's screens) and
the 10 providers the framework now seeds are removed; `Registry.js` and `RegistryServices.xml` keep
`check#OwnerReadiness` (the readiness contract over Accounting's setup, the catalog checks, Commerce's stores and an
organization's Constitution, for the organizations the viewer administers) and gain `get#OpenPaths` (which places open
to the viewer); `template/AdminRegistry.js` is what Admin's own screens read of the framework's services (`readiness`,
`overview`, `settings`, `openFixes`); `Admin.js` has `settingsRoutes` and the registries' other places in
`mountRoutes`; Health is Admin's own screen around the System screen's sections, SupportSummary is mounted by location
and its file deleted, SettingsHome includes the System screen at its own location; Home, Protocols › Home,
Organizations, OrganizationDetail and ReferenceData read through `AdminRegistry.js`, and Insights' `Settings.js`,
`Datasets.js` and DataReadiness call the framework's services. In mr-mantle: People's `get#Readiness` answers the
framework's contract (`subject`, `scope`, `fixRoute`).

| Operation | Answers or does | Checks |
|---|---|---|
| `get#Readiness` | the checks of the registered providers, each with its state, subject, scope, freshness, fix place as a key and the refresh offered; `failedProviders`, `ownerOptions`, `groupOptions`; narrowed by provider, owner, group, state, `scopeParameters` and `instanceOnly` | signed in; each provider's `requiredPermission`; the owner's service as the caller (a refusal leaves it out, a failure names it) |
| `request#Refresh` | one run of a registered operation's service with the registration's key, the operation key, the reason and the scope parameters the row names; `requested` or `failed` | signed in; the row exists and names a service (`REFRESH_UNAVAILABLE`); its `requiredPermission` (`REFRESH_DENIED`); the owner checks the scope |
| `get#Overview` | the ten worst checks that are not ready and four cards, each with the key of its place | `ADMIN_OVERVIEW_VIEW` |
| `get#SupportSummary` | the instance, the checks, the protocols and the effective settings; redacted on request | `ADMIN_OVERVIEW_VIEW`; each section the caller's own permissions |
| `find#Settings` | the entries the caller may open, with `destinationRoute` and `providerKey`; the apps and groups to choose from | signed in; each entry's `viewPermission` and its `screenLocation` open to the caller's own grants |
| `check#InstanceSetup` | one of the framework's ten checks by key | `ADMIN_OVERVIEW_VIEW` (the protocols: what `get#ProtocolReadiness` asks) |

| Screen | With no mount | Under Admin | Admin | What Admin keeps or adds | BOSS rows that name it | Raw part |
|---|---|---|---|---|---|---|
| Health | every provider the viewer reads; a fix place that is a System screen | the same with the organization filter | S | `choices`; the organization as `scopeParameters`, refused first when it is not the viewer's (`check#Record`); a fix link only where its place opens (`get#OpenPaths`); its own `requestRefresh`, which checks the organization before the framework's `request#Refresh` | none (the area's `BOSS_ADM_OPERATIONS`) | none |
| SupportSummary | the summary, print, `summaryJson` | the same | D | nothing | none | none |
| SettingsHome | the framework's entries, linked to System's screens | every BOSS app's entries by Admin's routes | W | nothing (it has no transition) | `BOSS_ADM_SETTINGS_INDEX`: the screen, a non-inheritable ALLOW for `BOSS_USERS` | none |

Where each check `Registry.js` made is now:

| Check | Now |
|---|---|
| a provider read only with its `requiredPermission` | `Readiness.js` `readiness`, by `ec.user.hasPermission`; Accounting's and Products' rows name none and `check#OwnerReadiness` asks it as the owners read theirs (an instance administrator holds every one, a Projects client none), F113 |
| a provider naming a missing service or permission, or disabled, left out and logged | `Readiness.js` `registered` |
| a provider that fails named, the others answering; one that refuses left out | `Readiness.js` `readiness` (the same refusal codes and wording, with the framework's "not authorized") |
| an organization outside the viewer's scope refused (readiness) | `Registry.js` `organizationsOf` in `check#OwnerReadiness` (`RECORD_UNAVAILABLE`, which leaves the provider out); Mantle's `get#Readiness` (`ORGANIZATION_DENIED`); Admin's Health refuses it before any provider is asked |
| an instance check left out when an organization is asked for | `readiness`: a provider that takes none of the scope asked for is left out |
| a refresh's permission; an unregistered refresh | `Readiness.js` `requestRefresh` |
| a refresh's organization within the viewer's scope | the owner's service, which is all a scope parameter reaches; Admin's Health `requestRefresh` asks `check#Record` first. No registered refresh takes an organization |
| only the registered scope parameters passed, never the operation key, the reason or the key | `requestRefresh` |
| a fix link only when the viewer opens it | under System the link widget disables a screen path the viewer does not open; under Admin `get#OpenPaths` (`Admin.js` `destinationOpens`, unchanged) through `AdminRegistry.js`, for Home, Health, OrganizationDetail and Protocols › Home |
| a health card's link only when the viewer opens it | the same, in `AdminRegistry.js` `overview` |
| the overview and the support summary for `ADMIN_OVERVIEW_VIEW` | `Readiness.js` `overview`, `supportSummary` |
| a settings entry only where its destination opens and its view permission holds | `Readiness.js` `settings`: `screenLocation` counted down its directory by the caller's own `ArtifactAuthzCheckView` rows (`Admin.js` `pathOpens`, ported), and `viewPermission` |
| the settings index for an Admin user or a BOSS user granted it, `ADMIN_UNAVAILABLE` otherwise | gone: any signed-in user is answered the entries their own grants open, none where they open none (F116) |
| Health's link on a settings entry only where Health opens | the System screen asks the framework for the link's path (`sri.buildUrl(...).getDisableLink()`) |
| the support summary's redaction | `Readiness.js` `supportSummary`, unchanged but for the JSON's `kind`, `moqui.admin.supportSummary`, and the checks' `organizationPartyId`, which is left out with their scope |

Findings:

- F109. The framework's six services are granted by an ALLOW that is not inheritable, as `call#TargetService` is: a
  registered service is then authorized for the caller as a direct call is, where Admin's registry was granted
  inheritably and carried its grant into every provider. Below Admin's screens nothing changes (the area's
  always-allow). Outside them a caller needs the provider's own grant: Insights' users hold their app's services by
  their root group, so `BOSS_INS_READINESS` and `BOSS_INS_READINESS_READ` (which granted Admin's registry services)
  are deleted, and `mantle.humanres.PeopleServices.get#Readiness` is a row of the `BOSS_ADMIN` artifact group, for
  the users who reached it through Admin's registry before. `refresh#DatasetIndex` asks its registration's permission
  itself now, since it is called directly by its app's users. An Insights reader's refresh is refused by the
  registration's permission (`REFRESH_DENIED`) where the registry itself was not theirs to call.
- F110. A scope is opaque to the framework. A request carries `scopeParameters`, a provider's or a refresh's row names
  those its service takes, and the per-organization loop is the service's: Mantle's People readiness already answered
  every employer in its caller's scope, and Admin's adapters loop the organizations the viewer administers. A check
  answers whom it is for (`subject`) and what narrows to it (`scope`), which Health sends back as "Check again" and as
  a refresh's scope. People's checks are therefore those of People's own scope for the caller, not of Admin's.
- F111. A place is a key everywhere: a provider's `fixRoute` and `fixId`, a check's own with `fixParameters`, a
  settings entry's `destinationRoute`, a card's `route`. The mount's routes resolve them, so System links the
  framework's screens and none of an app's, and Admin gives its apps' pages as plain routes. The fix places of the
  instance's own checks are delegated screens now where they were Tools' data grid: the mail server's is Mail relays
  and the report alert topic's is its TopicDetail; the report email template's is still Tools' grid
  (`emailTemplates`), which no delegated screen edits.
- F112. Whether a settings entry is shown is decided in the service, from the row: the mount is not in a service's
  context (F101), and a direct call must list no more than a screen does. So an entry names `screenLocation`, and the
  seven entries of the framework's System screens are seeded again in mr-boss under the same keys with Admin's
  screens (and owner Admin): with BOSS installed their place is Admin's for its delegates, and System shows them to a
  user who opens both. A location need not be a file (Admin's MailRelays is mounted by location): only its grants and
  those of the screens above it are read.
- F113. The framework asks a registration's permission with `ec.user.hasPermission`. BOSS's `has` gave an instance
  administrator every permission and a Projects client none; `ADMIN` holds `INSIGHTS_VIEW`, `INSIGHTS_DATASET_EDIT`,
  `PEOPLE_CONFIGURE`, `CONTACT_ADMIN` and `VoiceAdmin` by data, and not `ACCOUNTING_VIEW` or
  `PRODUCT_CATALOG_VIEW`, so those two providers' rows name no permission and Admin's contract service asks it with
  `has`. The owners of those two therefore show in Health's owner filter to a reader who gets none of their checks.
- F114. SupportSummary is mounted by location, not wrapped as the plan had it: no grant names it, its one transition
  calls only `get#SupportSummary`, which asks `ADMIN_OVERVIEW_VIEW`, and Admin adds nothing. `supportSummary` is a
  framework route now and left Admin's mount.
- F115. The System home's attention list: the framework's checks are registered as providers and no longer computed
  twice, so four items left `get#SystemAttention`'s own code (failed job runs, the backup, a silent cluster member,
  lost telemetry) and come from the readiness checks, with the checks it never had (the mail server, report delivery,
  administrator factors, protocols, the Constitution). What it still computes is what is no setup check: the resource
  readings, a failed listener, a draining instance, saturated workers or pool, and the counts of work that waits. A
  failed check is critical, a warning or unavailable one a warning; the ten worst are listed and the rest counted
  with a link to Health; a fix place is linked where it is a System screen. It asks `instanceOnly`: a provider whose
  registration names a scope (an organization's setup, a store) is Health's and its app's, not the instance's home's;
  with BOSS loaded those were 46 more rows and a quarter second of Products' catalog checks at every render of the
  home. The job runs check answers the list of the failed runs as its fix place (F7's parameters). The cluster check
  counts a member silent for three minutes, as the home did: its peers delete its row after ten, which Admin's ten
  minutes never reached. On an instance with telemetry off the home now says so (the check is unavailable), and an
  external database's instance is told its embedded backup job is paused, as Admin's Home was.
- F116. `find#Settings` is any signed-in user's and refuses nobody: it answers the entries the caller's own grants
  open. Admin's refused a user who was neither an Admin user nor a BOSS user granted the index (`ADMIN_UNAVAILABLE`);
  a Projects client is now answered an empty list. `BOSS_ADM_SETTINGS_INDEX` no longer names a registry service.
- F117. The framework's rows have owner System. Admin's Home asks the instance's own checks by that owner, and
  Health's owner filter shows System where it showed Admin for them.
- F118. `get#Overview` has no reader in Tools: the System home has its own counters and attention list, so the cards
  are Admin Home's alone, each link resolved by the mount and shown where it opens.
- F119. Health's "Check again" carried the row's own state, group and owner to the screen (F24: the target declares
  them), so a check that changed state vanished from its own recheck; the link sends them empty. The settings
  index's links to a screen send the index's find values empty for the same reason (Localization declares `text`).
- F120. Admin's Home is a screen of the root, not of an area, so a relative route is not followed from it: its links
  are the places' paths under `/qapps`, made by `AdminRegistry.js` from the mount's routes.
- F121. The redaction does not rewrite a registration's own title or a provider's name (it never did): they are
  their owners' seed text. A check's scope is left out of the summary with its fix place.
- F122. No recorded agent replay names a screen, a service or a sentence of this phase (not replayed).
- F123. For the finishing phase: mr-boss `plan/design-admin.md` §14 and §14.2 and `impl-admin.md` P2 and P19 describe
  the registries as Admin's; `design-insights` and `impl-insights` name `ironstone.admin.RegistryServices`. The
  acceptance `PeopleFoundationWalk` expects "People setup" on Health, which lists a check's own title (as it did). The
  support summary's JSON has a new `kind`. mr-l10n has no string of the three screens.

Tests: `./moq test tools --nobuild` (73 features; 3.4 and 3.5 s of 4 s in the last two runs, 3.5 to 4.0 in the three
before them; 5.0 before `instanceOnly`, when the home's render and the attention spec ran Products' catalog checks; no
render added: `SystemAttention` holds the failed job run as a check with its place and the merged order over checks of its
own, `AdminMounts`' static rule reads the three screens, at least 40 delegated and 56 raw); `./moq test framework
--nobuild --spec AdminReadiness --spec AdminConfiguration --spec AdminProtocols --spec AdminJobs --spec AdminRecovery
--spec AdminDataTransfer` (21 features, 0.7 s; `AdminReadiness`' four take 0.1 s, over registrations, users and
grants of its own: a failing provider named while the others answer, one not readable, one refusing, one naming no
service and a disabled one left out, a scope reaching only the provider that names it, `instanceOnly`; a refresh's
permission, a failing owner's safe result, an unregistered and a forged key; an entry by an allow of its own and
below an inheritable allow, and not below a deny, below an allow that is not inheritable, with no grant, permission,
place or screen; the overview's cards and the summary's redaction and refusal); `./moq test mr-boss --nobuild --spec
AdminRegistries --spec AdminAccess --spec AdminAudit --spec AdminSettings --spec AdminJobRuns --spec PeopleAccess
--spec InsightsHome --spec InsightsDatasets --spec InsightsSettings --spec AdminOrganizations --spec AdminRelays
--spec InsightsCatalog --spec AdminSystemMessages --spec AdminMailQueue --spec AdminConstitution --spec AdminExports
--spec AdminImports --spec InsightsAccess --spec AdminGroups --spec AdminUsers --spec AdminSecurity --spec
AdminDevices --spec AdminCapabilities --spec AdminLifecycle --spec AdminInvitations` (106 features, 24.8 s;
`AdminRegistries` keeps Admin's part: the contract service per organization, another organization refused, a place
open or not for the reader and for ADMIN, Health's refresh through Admin's route for the operator's organization and
refused for another, the BOSS entries of the index with the seven seeded again, and the renders of Home, Health and
the support summary through Admin's route); `./moq test mr-mantle --nobuild --spec PeopleScope` (5 features); `cargo
test -p moqui-host --test it tools_screens -- --ignored` (the three screens, the summary's JSON and the home's
attention list as ADMIN; 26 s); `moqui lint --screens` with no parse failure and no flagged site that is new (135, as
before), `moqui lint` of the six changed service files. Changed and not run: mr-boss's acceptance
`AdminRegistriesWalk` (the mail server's fix place; the finance reader refused Admin's contract service and answered
no Accounting check) and `AdminReviewWalk` (the two service names).

### Finishing (Nov 11, 2026)

State: every phase above is built and committed on branch `admin-screens-down` of mr-moqui, mr-boss, mr-mantle and
mr-marble. The finishing phase is three things:

- **The documents.** In mr-boss: `plan/design-admin.md` (§1.4 on which screens are the framework's and what Admin
  keeps, §13 without the Developer tools area, §14 with the registries as the framework's, and a line in each moved
  screen's section), `plan/impl-admin.md` §9, `plan/design-insights.md` §4.2 and `plan/impl-insights.md` §9,
  `plan/design-crosscut.md` (§3.30, §4.9's Z5 and Z9 rows, §5, §7, §8), the registry statements of `impl-people`,
  `impl-sales`, `impl-marketing` and `impl-maintenance`, two rows of `ss-inventory`, and the README. Here:
  [Practices-App-Dev](../docs/dev/Practices-App-Dev.md#a-framework-screen-under-an-apps-root) §5 (the three ways an
  app shows a framework screen, the grant and mount rules, and the engine behaviours of F16, F18, F24, F36, F101 and
  F102), [Attack-Surface](../docs/security/Attack-Surface.md#tools-and-system) ("Screens another app shows": the mount
  rule for both tracks and the engine's rules of F54 and F94), `plans/README.md`, and this plan's own tables read
  against the Record (the A4 row, `state()`'s `hidden`, the registries among what moved down).
- **The acceptance specs changed by path**, run one by one: mr-boss's `AdminFoundationWalk`, `AdminRegistriesWalk`,
  `AdminJobsUsable`, `AdminReviewWalk`, `AdminCapabilitiesUsable`, `AdminAuditUsable`, `InsightsDatasetsUsable`,
  `InsightsDeliveryUsable`, `InsightsAlertsUsable`, `InsightsWallDisplayUsable`, `InsightsReviewWalk`, and the sweeps
  whose screen lists changed (`AdminTui`, `AdminServerStatic`, `InsightsTui`, `InsightsServerStatic`); the framework's
  `acceptance/client/LoadBasic`; the opt-in `tui_screens` cargo test (A1). `PeopleFoundationWalk` reads "People
  setup" on Health (F123).
- **The full suites, once:** `./moq test-framework` and `./moq test-apps`.

Results:

- **Acceptance, one by one.** The fifteen of mr-boss pass. Three expectations written with the phases and not run then
  were corrected to what the screens do: `AdminAuditUsable` (a membership change's record is its group, the
  `adminGroup` route, not the user), `AdminJobsUsable` (the run's trace link is a screen path of System's, read under
  the web root as a browser asks for the page; under Admin as a spec's root it does not resolve) and
  `AdminRegistriesWalk` (an anchor's ampersand is escaped in html). The framework's `LoadBasic` passes. The opt-in
  `tui_screens` passes after two gallery snapshots took the registries' four enumeration types of A6 (a count, the
  first rows of a list). The acceptance specs over changed screens that the phases did not change were not run
  (`InsightsDrillUsable`, `AdminDevicesUsable`, `AdminExportsUsable`, `AdminImportsUsable`, `PeopleFoundationWalk`, the
  client `InsightsDashboardsUsable`).
- **`./moq test-framework`:** every suite passes but `cargo test --workspace`, by one test of `moqui-git`
  (`the_tree_refs_compare_and_merge_methods_through_services`: a git update's ECA finds no report service in that
  crate's runtime), which passes alone and failed in one of two more runs of the crate; nothing here touches
  `moqui-git` or `Git.gitecas.xml`. The specs: framework 532 features, 16.1 s of 17 s; tools 3.8 s of 4 s (3.3 to 3.6 s
  alone and warm, 4.8 and 5.4 s in the first two runs after a build).
- **`./moq test-apps`**, three runs: 3,626 features, 3,618 then 3,622 then 3,624 passing; the client suite passes each
  time. What failed, none of it this work's:
  - mr-mantle's `AccountingRecurringAging` and `SupportCases` fail while the UTC date is ahead of the local one (the
    evening: failed at 23:59:57, passed at 00:00:07).
  - `AdminDevices` found the device Mantle's `SharedDevices` spec leaves under the same name when a run puts both on
    one worker's database; its setup now removes its own names first, and it passes in the full run.
  - `PeopleAccess` ("No department is set up") and `PeoplePositions` (a pay grade "Pos spec shared grade") read rows
    other People specs leave on the same database; both pass alone. Left for the People work.
  - mr-boss's client suite reads 62 s of its 57 s budget; this work changed no client spec, and the People hire walk
    (6.6 s) is new since the budget was set.
- Known and not this work's, seen in the phases: `AdminAccessServices` "a membership change is seen at once by the
  session it changes" and `AdminBackups`' lock check each fail about one run in six; three `gate3` and four
  `gate3_browser` tests fail before and after (F22).
