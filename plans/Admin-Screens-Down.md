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
  parameters, plain}}` keyed by the registrations' `ownerRoute` and `linkKey` values, framework keys defaulting to
  relative paths; optional `hide` list.
- `reportPolicy` (beside `reportLinks`; nulled in `T/screen/Dashboards.xml` and `T/screen/System/Monitor.xml`, set in
  `B/screen/Insights.xml`): `features {alert, share}`, `range`, `datasetIdPrefix`, `datasetScopeParameter`,
  `datasetScopeLabel`, `datasetScopeHelp` (I1), `recipientPermissionId`, `reportEmailTemplateId`, `alertAudience`, `shareGroups`, `recipientCap`, `calendar`, each
  described in [Monitoring](../docs/tools/Monitoring.md#mounting-the-report-screens). `reportLinks` has the keys
  `alerts` and `alertShared` since I0; `wallDisplay` and a Tools default for `drill` come with their screens (I4).
- `T/template/ReportMount.js`: `state()` gives `{mounted, links, policy}`, every key and field present, and
  `url(key, own)` is a transition's response. It is the one reader of both maps.
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
| | Exports, DataExport | | D: Admin's own part was the permission check the services make (the record's A5) |
| | Imports | | S: organization filter |
| | ImportDetail | | S: the import read with the wizard's context |
| Audit | AuditChanges | all audit rows (old AuditLog list) | D: `find#Changes` alone carries the scope (the record's A4) |

Moving down: `B/template/AdminConnectionParts.xml` to `T/template/ConnectionParts.xml`; `getUserList` to
`T/template/UserChoiceTransitions.xml`; `CapabilityServices.get#MatrixContext`/`apply#MatrixChanges` to
`org.moqui.admin.AccessServices` (`AccessMatrix.js`); the generic parts of `Data.js` and `DataImport.js` to
`DataTransferExports.js` and `DataTransferImports.js`.
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
| A5 Data | `System/Data/**`; five `DataTransferServices` operations | area file, two screens by location, two S screens, Backups wrapper; `DataServices` deleted | `F/test/AdminDataExport`, `AdminDataTransfer`; `AdminExports`, `AdminImports` |
| A6 Registry (after track I; droppable) | `moqui.server.ReadinessProvider`, `RefreshOperation`, `SettingsEntry`; `org.moqui.admin.ReadinessServices`; framework seed rows; `get#SystemAttention` becomes the not-ready checks of `get#Readiness`; `Operations/Health`, `SupportSummary`, `Settings/SettingsHome` | entities and most of `Registry.js` deleted; the Mantle adapters become contract services the rows name (as People's `people.setup` is already); rows re-seeded | a framework registry spec; `SystemAttention`, `AdminRegistries`, `PeopleAccess` |

## Track I: Insights to Dashboards

Tools gets the generic screens, each reading and writing only through `ReportServices`, with presentation scripts in
`T/template/Report*.js`. A mountable screen never reads an entity directly.

| Dashboards screen | Change | Insights after |
|---|---|---|
| Datasets, DatasetDetail, DatasetFields, DatasetView | merge: the business list and stewardship on top; today's telemetry and index list stays as "Other datasets" where there is no mount | W (grants name locations and transitions; the record's I1) |
| Schedules, ScheduledReportDetail, ReportDelivery | new; `Reports.xml` keeps previews, the compose link and the owner-mode form | D; the area adds the recipient cap |
| Alerts, AlertRule, AlertShared | one list (Mine, Received, Templates, Kind) and one editor: a shared basis form, then Contact points or Audience; links from `reportLinks`; state through `get#AlertState` | W taking in only the business transitions |
| Drill, Analysis | new generic screens | S: scope, meaning, record links, cost filter; organization calendar |
| Dashboard | `share` transition and dialog (owner only; own groups plus `reportPolicy.shareGroups`) | W |
| Playlists, WallDisplay | new list, create and share screen; the hardened kiosk player | S (devices, display share); W |
| Explore, Split, Panels | the "no readable dataset" guard moves into Explore and Split, the default range into Explore (and Dashboard); Panels carries its menu image | D (the record's I0) |

New `ReportServices` operations (`F/service/org/moqui/report/ReportReads.js`): `get#BusinessDataset`,
`get#BusinessDatasetOptions`, `find#ReportRecipients`, `find#ScreenSchedules`, `store#ScreenSchedule`; `ownerName` in
the answers of `get#ReportOccurrence` and `get#AlertState`. Five artifact rows are added to existing BOSS groups.

Stays in BOSS: the Insights root, Home, the Reports catalog, Records, Flows, Settings, AddToDashboard, SavedReports,
ManagementDigest, and the wrappers Dashboards, DashboardEditor and PanelDetail.

| Phase | mr-moqui | mr-boss | Specs |
|---|---|---|---|
| I0 Contract | `reportPolicy` nulled at `Dashboards.xml` and `System/Monitor.xml`; `ReportMount.js`, which every Dashboards screen reads the links through; Explore and Split guard; Monitoring.md | root sets `reportPolicy`; direct-mount Panels, Explore, Split | `ReportMounts`, `InsightsAccess` |
| I1 Datasets | two operations; four screens; `ReportDatasets.js`; a scope policy registers its parameter; `create#BusinessDataset` asks the named dataset's update artifact | four wrappers; two rows; `DatasetServices` keeps readiness and refresh | `BusinessDatasets`; `InsightsDatasets` |
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
