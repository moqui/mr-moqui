# MR Moqui — UI review: System administration

**Status — 2026-11-25.** Both historical findings passed the [SYS-B02 recheck](#sys-b02--post-cleanup-recheck-2026-11-25). The active exporter had dropped records, and the conf filter retained its accessible name. The wider administration review remains in progress.

**Original review — 2026-11-16:** review pass started. Batch 01 exercised bounded read-only administration paths in Material
light; two findings await consolidated triage. No activity is complete, no refinement was made and no automated
tests were run.

Contents: [scope](#1-purpose-scope-and-sources) · [method](#2-review-method-and-refinement-criteria) ·
[inventory](#3-screen-and-surface-inventory) · [activities](#4-end-to-end-activities) ·
[verification](#5-existing-checks-and-future-verification) · [record](#6-coverage-findings-and-running-record) ·
[decisions](#7-decisions).

## 1. Purpose, scope and sources

Review the instance administrator's daily work: establish what needs attention, change the right configuration or
access rule, inspect the result and retain evidence. Follow operations through their runs and outputs rather than
stopping at a settings form. This is the core System plan; Monitor and AI/integrations have companion inventories.

Sources: [tools README](../runtime/components/base/tools/README.md),
[administration-screen extraction](archive/Admin-Screens-Down.md), [System/Tools homes](archive/Tools-Fancy.md),
[component deployment](../docs/tools/Component-Deploy.md), [Run and Deploy](../docs/Run-Deploy.md),
[archived review](archive/UI-Refinement.md), current `tools/screen/System`, `tools/template/Admin*.js`,
`AccessReview.js`, `HomeCatalog.js`, framework `org.moqui.admin` services and their registrations/security data.

### Ownership and handoffs

| Surface | Owner / handoff |
| --- | --- |
| System root/Home and core administration routes in §3 | This plan |
| Monitor and diagnostic behavior reused by Conf/Logging | [Monitor](ui-review-system-monitoring.md); SYS-06 checks the entry and return |
| Llm, Constitution, Integrations, Protocols, EdgeClients/EdgeNode, EntitySync, Voice/ChatBridges | [AI/integrations](ui-review-system-ai-integrations.md) |
| Database manipulation and maintenance | [Tools](ui-review-tools.md); System owns registered import/export/backup orchestration |
| Login, user-facing terminal/device flows | [Shared/Gallery](ui-review-shared-gallery.md); this plan owns administration and its observable consequences |
| Business-app administration mounts | Excluded addons; AdminMounts' framework fixtures supply supporting contract evidence |

System's `adminMount=null` exposes instance-administrator parts only through the actual System grant. Distinguish
registered administration from raw artifact/entity controls; they are not interchangeable authority paths.

## 2. Review method and refinement criteria

Apply the [common review protocol](ui-review-shared-gallery.md#common-review-protocol) and
[verification policy](ui-review-shared-gallery.md#verification-policy-for-later-execution). The details below add this
area's fixtures, dependencies and judgment criteria.

### Environment, identities and records

- Prepare `review.admin` for instance administration, `sec.view.only` for view-only System and `sec.none.only` for
  refusal. Prepare narrow registered-admin users from the actual permissions used by each `org.moqui.admin` service;
  record those grants, rather than assigning ADMIN and calling the result limited access.
- `CONF_EDIT`, `COMPONENT_DEPLOY`, `TERMINAL_HOST`, `DATABASE_ADMIN` and `DATABASE_EXTERNAL` are explicit capabilities;
  several are sealed and normally granted through ADMIN_ADV. Grant only the one needed for its positive counterpart.
  Check `SecurityTypeData.xml` and `ToolsSecurityData.xml` at execution.
- Fixtures: dedicated account/group and row-filter binding; local identity provider; harmless registered job with
  success/partial/failure runs; notification topic; mixed import results; registered export; backup job; local Git
  resource; disposable hosted instance configuration; local print sink and certificate fixtures. Prepare a second
  unmodified administration path before access/session exercises.
- Copy configuration and deploy only a disposable component/root. Component removal, restart, hosted-instance
  lifecycle, backup and certificate exercises get a separate runtime when they could disrupt other reviews.
  No real printer, external certificate authority, remote host or production resource is a fixture.
- Check enabled backend capabilities before each activity. Inspect an unavailable state when the capability is
  absent, but leave successful provisioning/printing/provider branches blocked. Restore preferences, grants,
  notifications, registered jobs and local integrations after their consumers finish.

The registered-admin counterpart users below each receive a System route grant plus only the permissions listed
for their task, from `framework/data/SecurityTypeData.xml`. Prepare a view-only counterpart without the write/run
permission. These do not grant the raw instance-administration controls or bypass registered-owner restrictions.

| Proposed identity | Explicit permissions / activity |
| --- | --- |
| `review.system.health` | `ADMIN_OVERVIEW_VIEW`; SYS-01, with a refresh provider's own permission only for its positive refresh case |
| `review.system.identity` | `ADMIN_IDENTITY_VIEW`, `ADMIN_IDENTITY_MANAGE`; SYS-02/04. `ADMIN_IDENTITY_GLOBAL` is a separate sealed positive for cross-scope account changes |
| `review.system.access` | `ADMIN_ACCESS_VIEW`, `ADMIN_ACCESS_MANAGE`; SYS-03. Sealed grants still require their own holder |
| `review.system.settings` | `ADMIN_SETTINGS_VIEW`, `ADMIN_SETTINGS_MANAGE`; SYS-05. `CONF_EDIT` is separately prepared for SYS-06 |
| `review.system.operations` | `ADMIN_OPERATION_VIEW`, `ADMIN_OPERATION_MANAGE`; SYS-10/11, within the job/topic registration |
| `review.system.transfer` | `ADMIN_TRANSFER_VIEW`, `ADMIN_TRANSFER_RUN`; SYS-12/13, with target-specific authority |
| `review.system.backup` | `ADMIN_BACKUP_VIEW`, `ADMIN_BACKUP_RUN`; SYS-14. `ADMIN_BACKUP_DOWNLOAD` is separate and sealed if an archive download is offered |
| `review.system.audit` | `ADMIN_AUDIT_VIEW`, with `ADMIN_ACCESS_VIEW` for AccessReview; SYS-15 |

### Area-specific criteria

- Home's identity, readiness, counts and attention links must agree with their destination. A missing metric says
  unavailable, not zero; one failed provider does not hide independent health information.
- Changes show effective value, source, scope, revision and reload/restart effect. Raw controls identify their broader
  consequence. A stale preview cannot silently change a different policy/grant/record.
- Run, queue and delivery states distinguish requested, executing, partial, failed and complete. Independent item
  errors remain named and retriable without replaying successful items.
- Credentials, recovery values and personal data are redacted appropriately in forms, audit/support outputs and
  downloads. Destructive actions name their target and preserve an available administrative recovery path.

### Ordered waves

| Wave | Activities | Continuation |
| --- | --- | --- |
| 1. Understand and control access | SYS-01–04 | Readiness → identity/grants → actual sign-in/access |
| 2. Configure the instance | SYS-05–09 | Preferences/conf/components/instances → effective behavior |
| 3. Operate jobs and data | SYS-10–14 | Runs/topics/import/export/backup → evidence |
| 4. Inspect and administer resources | SYS-15–19 | Audit, Git, printing, sessions and certificates |
| 5. Reconcile | SYS-20 | Remaining routes/modes and companion handoffs |

Keep account/grant fixtures intact through SYS-15 evidence review. Do not consume the sole successful backup or
remove the disposable component until its dependent output/reload checks finish. Serialize restarts last.

## 3. Screen and surface inventory

This inventory accounts for **78 XML definitions** assigned to this file; screen containers and helper
definitions are included. Routes below are Material paths unless labelled otherwise; app routes have Standard
`/apps` counterparts. A file's physical presence does not prove access or a supported interaction in every mode.
Each row maps to §4; helper inspection does not replace its parent workflow. Counts describe definitions, not user
activities. Dialogs, templates and non-XML surfaces follow the route tables.

### tools: `System`

| Route / surface | XML source | Default / variants / boundary | Activities |
| --- | --- | --- | --- |
| `/qapps/system/Access` | [Access.xml](../runtime/components/base/tools/screen/System/Access.xml) | Default: `Groups`. | SYS-03, SYS-15 |
| `/qapps/system/Audit` | [Audit.xml](../runtime/components/base/tools/screen/System/Audit.xml) | Default: `AuditChanges`. | SYS-15 |
| `/qapps/system/Cache` | [Cache.xml](../runtime/components/base/tools/screen/System/Cache.xml) | Default: `CacheList`. | SYS-09 |
| `/qapps/system/Certificates` | [Certificates.xml](../runtime/components/base/tools/screen/System/Certificates.xml) | Authored text: `html`, `qvt`, `tui`. | SYS-19 |
| `/qapps/system/Components` | [Components.xml](../runtime/components/base/tools/screen/System/Components.xml) | Context: `dataName`, `dirtyName`, `dirtyChanges`. | SYS-07 |
| `/qapps/system/Conf` | [Conf.xml](../runtime/components/base/tools/screen/System/Conf.xml) | Default: `View`. | SYS-06 |
| `/qapps/system/Data` | [Data.xml](../runtime/components/base/tools/screen/System/Data.xml) | Default: `Imports`. | SYS-12, SYS-13, SYS-14 |
| `/qapps/system/Home` | [Home.xml](../runtime/components/base/tools/screen/System/Home.xml) | Authored text: `html`, `qvt`, `tui`. Context: `period`, `refresh`. | SYS-01, SYS-09 |
| `/qapps/system/Instances` | [Instances.xml](../runtime/components/base/tools/screen/System/Instances.xml) | Default: `Fleet`. | SYS-08 |
| `/qapps/system/Operations` | [Operations.xml](../runtime/components/base/tools/screen/System/Operations.xml) | Default: `Jobs`. | SYS-01, SYS-10, SYS-11 |
| `/qapps/system/Print` | [Print.xml](../runtime/components/base/tools/screen/System/Print.xml) | Default: `Printer`. | SYS-17 |
| `/qapps/system/Resource` | [Resource.xml](../runtime/components/base/tools/screen/System/Resource.xml) | Default: `GitRepositoryList`. | SYS-16 |
| `/qapps/system/Security` | [Security.xml](../runtime/components/base/tools/screen/System/Security.xml) | Default: `UserAccount`. | SYS-02, SYS-04 |
| `/qapps/system/Settings` | [Settings.xml](../runtime/components/base/tools/screen/System/Settings.xml) | Default: `InstanceSettings`. | SYS-05 |
| `/qapps/system/TerminalSessions` | [TerminalSessions.xml](../runtime/components/base/tools/screen/System/TerminalSessions.xml) | Inspect the route and its visible controls through the activity. | SYS-18 |
| `/qapps/system/ThreadList` | [ThreadList.xml](../runtime/components/base/tools/screen/System/ThreadList.xml) | Context: `show`. | SYS-09 |
| `/qapps/system/Visit` | [Visit.xml](../runtime/components/base/tools/screen/System/Visit.xml) | Default: `VisitList`. | SYS-15 |

### tools: `System/Access`

| Route / surface | XML source | Default / variants / boundary | Activities |
| --- | --- | --- | --- |
| `/qapps/system/Access/AccessReview` | [AccessReview.xml](../runtime/components/base/tools/screen/System/Access/AccessReview.xml) | Hidden/helper entry; enter through its parent action and inspect direct access. Context: `groups`, `appName`, `asOfDate`. | SYS-03, SYS-15 |
| `/qapps/system/Access/ArtifactGroupDetail` | [ArtifactGroupDetail.xml](../runtime/components/base/tools/screen/System/Access/ArtifactGroupDetail.xml) | Hidden/helper entry; enter through its parent action and inspect direct access. Context: `artifactGroupId`. | SYS-03, SYS-15 |
| `/qapps/system/Access/ArtifactGroups` | [ArtifactGroups.xml](../runtime/components/base/tools/screen/System/Access/ArtifactGroups.xml) | Inspect the route and its visible controls through the activity. | SYS-03, SYS-15 |
| `/qapps/system/Access/Capabilities` | [Capabilities.xml](../runtime/components/base/tools/screen/System/Access/Capabilities.xml) | Authored text: `html`, `qvt`, `tui`. Context: `appName`, `groups`, `kind`, `text`, `view`, `changes`. | SYS-03, SYS-15 |
| `/qapps/system/Access/GroupDetail` | [GroupDetail.xml](../runtime/components/base/tools/screen/System/Access/GroupDetail.xml) | Authored text: `html`, `qvt`, `tui`. Hidden/helper entry; enter through its parent action and inspect direct access. Context: `userGroupId`, `grantAction`, `grantBundleGroupId`, `grantItem`. | SYS-03, SYS-15 |
| `/qapps/system/Access/Groups` | [Groups.xml](../runtime/components/base/tools/screen/System/Access/Groups.xml) | Authored text: `html`, `qvt`, `tui`. Context: `text`, `appName`, `kind`, `userPermissionId`. | SYS-03, SYS-15 |
| `/qapps/system/Access/RowRules` | [RowRules.xml](../runtime/components/base/tools/screen/System/Access/RowRules.xml) | Authored text: `html`, `qvt`, `tui`. Context: `userGroupId`, `artifactGroupId`, `entityFilterSetId`, `state`, `pageIndex`. | SYS-03, SYS-15 |

### tools: `System/Audit`

| Route / surface | XML source | Default / variants / boundary | Activities |
| --- | --- | --- | --- |
| `/qapps/system/Audit/AuditChanges` | [AuditChanges.xml](../runtime/components/base/tools/screen/System/Audit/AuditChanges.xml) | Context: `category`, `auditProjectionId`, `changedByUserId`, `recordKey`, `correlation`, `fromDate`, `thruDate`, `madeBy`, `pageIndex`, `pageNoLimit`, `log`. | SYS-15 |

### tools: `System/Cache`

| Route / surface | XML source | Default / variants / boundary | Activities |
| --- | --- | --- | --- |
| `/qapps/system/Cache/CacheElements` | [CacheElements.xml](../runtime/components/base/tools/screen/System/Cache/CacheElements.xml) | Context: `cacheName`. | SYS-09 |
| `/qapps/system/Cache/CacheList` | [CacheList.xml](../runtime/components/base/tools/screen/System/Cache/CacheList.xml) | Inspect the route and its visible controls through the activity. | SYS-09 |

### tools: `System/Conf`

| Route / surface | XML source | Default / variants / boundary | Activities |
| --- | --- | --- | --- |
| `/qapps/system/Conf/Changes` | [Changes.xml](../runtime/components/base/tools/screen/System/Conf/Changes.xml) | Inspect the route and its visible controls through the activity. | SYS-06 |
| `/qapps/system/Conf/Edit` | [Edit.xml](../runtime/components/base/tools/screen/System/Conf/Edit.xml) | Companions: [Edit.qvue](../runtime/components/base/tools/screen/System/Conf/Edit.qvue). Authored text: `html,qvt`, `qvue`, `tui`. Context: `fileIndex`, `line`. Declared modes: `vue,qvue,tui`. | SYS-06 |
| `/qapps/system/Conf/EditTui` | [EditTui.xml](../runtime/components/base/tools/screen/System/Conf/EditTui.xml) | Authored text: `tui`. Hidden/helper entry; enter through its parent action and inspect direct access. Context: `fileIndex`. | SYS-06 |
| `/qapps/system/Conf/Logging` | [Logging.xml](../runtime/components/base/tools/screen/System/Conf/Logging.xml) | Wrapper of Monitor/Settings; MON-10 owns edits, SYS-06 owns entry/return. Includes `component://tools/screen/System/Monitor/Settings.xml`. Context: `changeKey`. | SYS-06 |
| `/qapps/system/Conf/View` | [View.xml](../runtime/components/base/tools/screen/System/Conf/View.xml) | Companions: [View.qvue](../runtime/components/base/tools/screen/System/Conf/View.qvue). Authored text: `qvue`, `tui`. Declared modes: `vue,qvue,tui`. | SYS-06 |

### tools: `System/Data`

| Route / surface | XML source | Default / variants / boundary | Activities |
| --- | --- | --- | --- |
| `/qapps/system/Data/Backups` | [Backups.xml](../runtime/components/base/tools/screen/System/Data/Backups.xml) | Authored text: `html`, `qvt`, `tui`. Context: `pageIndex`. | SYS-14 |
| `/qapps/system/Data/DataExport` | [DataExport.xml](../runtime/components/base/tools/screen/System/Data/DataExport.xml) | Context: `targetId`, `scopeKey`, `option1`, `option2`, `option3`, `fields`, `review`. | SYS-13 |
| `/qapps/system/Data/Exports` | [Exports.xml](../runtime/components/base/tools/screen/System/Data/Exports.xml) | Authored text: `html`, `qvt`, `tui`. Context: `kind`, `targetId`, `state`, `fromDate`, `thruDate`, `pageIndex`. | SYS-13 |
| `/qapps/system/Data/ImportDetail` | [ImportDetail.xml](../runtime/components/base/tools/screen/System/Data/ImportDetail.xml) | Hidden/helper entry; enter through its parent action and inspect direct access. Context: `targetId`, `operationKey`, `pageIndex`, `errorsOnly`. | SYS-12 |
| `/qapps/system/Data/Imports` | [Imports.xml](../runtime/components/base/tools/screen/System/Data/Imports.xml) | Authored text: `html`, `qvt`, `tui`. Context: `targetId`, `state`, `fromDate`, `thruDate`, `pageIndex`. | SYS-12 |

### tools: `System/Instances`

| Route / surface | XML source | Default / variants / boundary | Activities |
| --- | --- | --- | --- |
| `/qapps/system/Instances/DatabaseHosts` | [DatabaseHosts.xml](../runtime/components/base/tools/screen/System/Instances/DatabaseHosts.xml) | New database host dialog (`store#DatabaseHost`); its admin login is a vault item, named by its binding. | SYS-08 |
| `/qapps/system/Instances/Environments` | [Environments.xml](../runtime/components/base/tools/screen/System/Instances/Environments.xml) | New environment dialog (`store#InstanceEnvironment`); Deploy posts `request#InstanceDeploy`, which runs now or opens Vaults › Lease. | SYS-08 |
| `/qapps/system/Instances/Fleet` | [Fleet.xml](../runtime/components/base/tools/screen/System/Instances/Fleet.xml) | Read only, rows from `get#InstanceFleet`; without INSTANCE_ADMIN a notice and no rows. Links to InstanceDetail and Monitor Services and Releases. | SYS-08 |
| `/qapps/system/Instances/Hosts` | [Hosts.xml](../runtime/components/base/tools/screen/System/Instances/Hosts.xml) | New host dialog (`store#InstanceHost`): unix socket, https with a client certificate, or http to this machine only; the client key stays a vault item. | SYS-08 |
| `/qapps/system/Instances/Images` | [Images.xml](../runtime/components/base/tools/screen/System/Instances/Images.xml) | New image dialog (`store#InstanceImage`); a registry login is a vault item, leased by the provision that pulls it. | SYS-08 |
| `/qapps/system/Instances/InstanceDetail` | [InstanceDetail.xml](../runtime/components/base/tools/screen/System/Instances/InstanceDetail.xml) | Context: `appInstanceId`. | SYS-08 |
| `/qapps/system/Instances/InstanceList` | [InstanceList.xml](../runtime/components/base/tools/screen/System/Instances/InstanceList.xml) | Hidden/helper entry; enter through its parent action and inspect direct access. | SYS-08 |
| `/qapps/system/Instances/Kinds` | [Kinds.xml](../runtime/components/base/tools/screen/System/Instances/Kinds.xml) | Read only: the seed kinds with their secrets and dependencies. | SYS-08 |
| `/qapps/system/Instances/Rotation` | [Rotation.xml](../runtime/components/base/tools/screen/System/Instances/Rotation.xml) | Context: `environmentId`. Rotate due and Rotate all post `request#InstanceRotation`, which runs now under machine access or opens Vaults › Lease. | SYS-08 |

### tools: `System/Operations`

| Route / surface | XML source | Default / variants / boundary | Activities |
| --- | --- | --- | --- |
| `/qapps/system/Operations/Health` | [Health.xml](../runtime/components/base/tools/screen/System/Operations/Health.xml) | Authored text: `html`, `qvt`, `tui`. Context: `healthGroupEnumId`, `state`, `ownerApp`, `providerKey`. | SYS-01 |
| `/qapps/system/Operations/JobDetail` | [JobDetail.xml](../runtime/components/base/tools/screen/System/Operations/JobDetail.xml) | Authored text: `html`, `qvt`, `tui`. Hidden/helper entry; enter through its parent action and inspect direct access. Context: `jobName`. | SYS-10 |
| `/qapps/system/Operations/JobRunDetail` | [JobRunDetail.xml](../runtime/components/base/tools/screen/System/Operations/JobRunDetail.xml) | Authored text: `html`, `qvt`, `tui`. Hidden/helper entry; enter through its parent action and inspect direct access. Context: `jobRunId`. | SYS-10 |
| `/qapps/system/Operations/JobRuns` | [JobRuns.xml](../runtime/components/base/tools/screen/System/Operations/JobRuns.xml) | Authored text: `html`, `qvt`, `tui`. Context: `jobName`, `state`, `fromDate`, `thruDate`, `pageIndex`. | SYS-10 |
| `/qapps/system/Operations/Jobs` | [Jobs.xml](../runtime/components/base/tools/screen/System/Operations/Jobs.xml) | Authored text: `html`, `qvt`, `tui`. Context: `state`, `ownerApp`, `term`, `pageIndex`. | SYS-10 |
| `/qapps/system/Operations/SupportSummary` | [SupportSummary.xml](../runtime/components/base/tools/screen/System/Operations/SupportSummary.xml) | Hidden/helper entry; enter through its parent action and inspect direct access. | SYS-01 |
| `/qapps/system/Operations/TopicDetail` | [TopicDetail.xml](../runtime/components/base/tools/screen/System/Operations/TopicDetail.xml) | Hidden/helper entry; enter through its parent action and inspect direct access. Context: `topic`. | SYS-11 |
| `/qapps/system/Operations/Topics` | [Topics.xml](../runtime/components/base/tools/screen/System/Operations/Topics.xml) | Context: `term`, `pageIndex`. | SYS-11 |

### tools: `System/Print`

| Route / surface | XML source | Default / variants / boundary | Activities |
| --- | --- | --- | --- |
| `/qapps/system/Print/PrintJob` | [PrintJob.xml](../runtime/components/base/tools/screen/System/Print/PrintJob.xml) | Default: `PrintJobList`. | SYS-17 |
| `/qapps/system/Print/Printer` | [Printer.xml](../runtime/components/base/tools/screen/System/Print/Printer.xml) | Default: `PrinterList`. | SYS-17 |

### tools: `System/Print/PrintJob`

| Route / surface | XML source | Default / variants / boundary | Activities |
| --- | --- | --- | --- |
| `/qapps/system/Print/PrintJob/PrintJobDetail` | [PrintJobDetail.xml](../runtime/components/base/tools/screen/System/Print/PrintJob/PrintJobDetail.xml) | Context: `printJobId`. | SYS-17 |
| `/qapps/system/Print/PrintJob/PrintJobList` | [PrintJobList.xml](../runtime/components/base/tools/screen/System/Print/PrintJob/PrintJobList.xml) | Hidden/helper entry; enter through its parent action and inspect direct access. | SYS-17 |

### tools: `System/Print/Printer`

| Route / surface | XML source | Default / variants / boundary | Activities |
| --- | --- | --- | --- |
| `/qapps/system/Print/Printer/PrinterList` | [PrinterList.xml](../runtime/components/base/tools/screen/System/Print/Printer/PrinterList.xml) | Hidden/helper entry; enter through its parent action and inspect direct access. | SYS-17 |

### tools: `System/Resource`

| Route / surface | XML source | Default / variants / boundary | Activities |
| --- | --- | --- | --- |
| `/qapps/system/Resource/GitRepositoryDetail` | [GitRepositoryDetail.xml](../runtime/components/base/tools/screen/System/Resource/GitRepositoryDetail.xml) | Hidden/helper entry; enter through its parent action and inspect direct access. Context: `repositoryId`. | SYS-16 |
| `/qapps/system/Resource/GitRepositoryList` | [GitRepositoryList.xml](../runtime/components/base/tools/screen/System/Resource/GitRepositoryList.xml) | Inspect the route and its visible controls through the activity. | SYS-16 |

### tools: `System/Security`

| Route / surface | XML source | Default / variants / boundary | Activities |
| --- | --- | --- | --- |
| `/qapps/system/Security/IdentityProviderDetail` | [IdentityProviderDetail.xml](../runtime/components/base/tools/screen/System/Security/IdentityProviderDetail.xml) | Authored text: `html`, `qvt`, `tui`. Hidden/helper entry; enter through its parent action and inspect direct access. Context: `authFlowId`. | SYS-04 |
| `/qapps/system/Security/IdentityProviders` | [IdentityProviders.xml](../runtime/components/base/tools/screen/System/Security/IdentityProviders.xml) | Authored text: `html`, `qvt`, `tui`. Context: `text`, `authFlowTypeEnumId`, `state`. | SYS-04 |
| `/qapps/system/Security/Sessions` | [Sessions.xml](../runtime/components/base/tools/screen/System/Security/Sessions.xml) | Context: `text`, `recent`, `visits`. | SYS-04, SYS-15 |
| `/qapps/system/Security/SignInPolicy` | [SignInPolicy.xml](../runtime/components/base/tools/screen/System/Security/SignInPolicy.xml) | Authored text: `html`, `qvt`, `tui`. Context: `text`, `rulesOnly`, `reviewGroupId`, `reviewFactor`, `reviewIp`. | SYS-04 |
| `/qapps/system/Security/UserAccount` | [UserAccount.xml](../runtime/components/base/tools/screen/System/Security/UserAccount.xml) | Default: `UserAccountList`. | SYS-02 |
| `/qapps/system/Security/WebProtection` | [WebProtection.xml](../runtime/components/base/tools/screen/System/Security/WebProtection.xml) | Inspect the route and its visible controls through the activity. | SYS-19 |

### tools: `System/Security/UserAccount`

| Route / surface | XML source | Default / variants / boundary | Activities |
| --- | --- | --- | --- |
| `/qapps/system/Security/UserAccount/UserAccountDetail` | [UserAccountDetail.xml](../runtime/components/base/tools/screen/System/Security/UserAccount/UserAccountDetail.xml) | Context: `userId`. | SYS-02, SYS-15 |
| `/qapps/system/Security/UserAccount/UserAccountList` | [UserAccountList.xml](../runtime/components/base/tools/screen/System/Security/UserAccount/UserAccountList.xml) | Hidden/helper entry; enter through its parent action and inspect direct access. | SYS-02 |

### tools: `System/Security/UserAccount/UserAccountDetail`

| Route / surface | XML source | Default / variants / boundary | Activities |
| --- | --- | --- | --- |
| `/qapps/system/Security/UserAccount/UserAccountDetail/VerifyEmail` | [VerifyEmail.xml](../runtime/components/base/tools/screen/System/Security/UserAccount/UserAccountDetail/VerifyEmail.xml) | Hidden/helper entry; enter through its parent action and inspect direct access. Context: `userId`, `factorId`. | SYS-02, SYS-04 |
| `/qapps/system/Security/UserAccount/UserAccountDetail/VerifySms` | [VerifySms.xml](../runtime/components/base/tools/screen/System/Security/UserAccount/UserAccountDetail/VerifySms.xml) | Hidden/helper entry; enter through its parent action and inspect direct access. Context: `userId`, `factorId`. | SYS-02, SYS-04 |
| `/qapps/system/Security/UserAccount/UserAccountDetail/VerifyTotp` | [VerifyTotp.xml](../runtime/components/base/tools/screen/System/Security/UserAccount/UserAccountDetail/VerifyTotp.xml) | Hidden/helper entry; enter through its parent action and inspect direct access. Context: `factorId`, `userId`. | SYS-02, SYS-04 |

### tools: `System/Settings`

| Route / surface | XML source | Default / variants / boundary | Activities |
| --- | --- | --- | --- |
| `/qapps/system/Settings/InstanceSettings` | [InstanceSettings.xml](../runtime/components/base/tools/screen/System/Settings/InstanceSettings.xml) | Inspect the route and its visible controls through the activity. | SYS-05 |
| `/qapps/system/Settings/Localization` | [Localization.xml](../runtime/components/base/tools/screen/System/Settings/Localization.xml) | Context: `kind`, `locale`, `text`, `missing`, `targetKey`, `pageIndex`, `rows`. | SYS-05 |
| `/qapps/system/Settings/SettingsHome` | [SettingsHome.xml](../runtime/components/base/tools/screen/System/Settings/SettingsHome.xml) | Context: `text`, `ownerApp`, `settingsGroupEnumId`. | SYS-05 |

### tools: `System/Visit`

| Route / surface | XML source | Default / variants / boundary | Activities |
| --- | --- | --- | --- |
| `/qapps/system/Visit/VisitDetail` | [VisitDetail.xml](../runtime/components/base/tools/screen/System/Visit/VisitDetail.xml) | Hidden/helper entry; enter through its parent action and inspect direct access. Context: `visitId`. | SYS-15 |
| `/qapps/system/Visit/VisitList` | [VisitList.xml](../runtime/components/base/tools/screen/System/Visit/VisitList.xml) | Inspect the route and its visible controls through the activity. | SYS-15 |

### tools: `roots`

| Route / surface | XML source | Default / variants / boundary | Activities |
| --- | --- | --- | --- |
| `/qapps/system` | [System.xml](../runtime/components/base/tools/screen/System.xml) | Default: `Home`. | SYS-01, SYS-20 |

### Embedded surfaces, dialogs, outputs and reconciliation

| Supporting surface | Activity / inspection |
| --- | --- |
| Home identity/counters, Needs attention, resource meters and report embed | SYS-01/09; monitoring owns chart diagnostics |
| BasicComms [System Home extension](../runtime/components/base/basic-comms/screen-extend/tools/screen/System/Home.xml) | SYS-01/20; Voice/Chat Bridges entries supplement the base Home catalog when basic-comms is loaded; AI-16–18 own their destinations |
| UserAccountDetail factors, preferences, memberships, audit, tarpit and login-as controls; VerifyEmail/Sms/Totp children | SYS-02/04/15; factors verify the selected user/context, not the lifecycle owner's |
| GroupDetail registered grants versus raw permission/authz/tarpit forms; Capabilities preview/apply; RowRules binding | SYS-03; evidence in AccessReview and AuditChanges |
| IdentityProviderDetail OIDC/OAuth/SAML configuration, mapping, test/enable; policy preview/confirm and session revoke | SYS-04; local provider required for success |
| [UserChoiceTransitions.xml](../runtime/components/base/tools/template/UserChoiceTransitions.xml): getUserList | SYS-03/10/20; selected users outside the choice page retain their identity/label; consuming administration and report forms keep their own access checks |
| Conf View/Edit QVue, EditTui, validation/diff, Changes pending/restart; Logging include of Monitor/Settings | SYS-06; MON-10 owns diagnostic settings behavior |
| Components preview/refusals, staged deploy/load/data/update/remove/restart controls | SYS-07; source/edit fixture separate from linked components |
| Instance environment/volume/link forms and lifecycle controls | SYS-08; inspect the implemented hosting backend, not an unbuilt fleet design |
| Jobs raw parameters/recipients versus registered configuration; run evidence/failed items; topic defaults/subscriptions | SYS-10–11; partial success and masked parameters |
| ImportDetail row retry/results; DataExport field selection/review/download; Backups coverage/exclusions/archive | SYS-12–14; System has no local import staging wizard or generic restore screen |
| AuditChanges raw `log=Y`, Sessions `visits=Y`, Localization `rows=Y` | SYS-05/15; alternate list modes replace the normal list and reset pagination/context |
| Git export/import/LFS/mirror; printer job download/hold/release/cancel; terminal End dialog; ACME confirmation | SYS-16–19; actual files/results, local prerequisites and confirmation identity |

Older standalone user/job/reference routes and Monitor launcher entries are not restored. New registered
administration areas are current scope. [Vault/Instances](Vault-Instances.md) and [Sealed Secrets](Sealed-Secrets.md)
remain separate proposals; their absent controls are not review failures or implementation requests.

## 4. End-to-end activities

Use visible controls and inspect intermediate states, confirmation and the resulting data/output. Unless narrowed
below, each activity has Material light/dark, actual supported TUI and a brief Standard/output branch. Select narrow
and keyboard checks where layout/interaction differs. Record each unattempted branch in §6; a service call or render
alone does not complete the activity. Names beginning `review.` are fixture proposals, not shipped accounts.

### SYS-01 — Assess the instance and follow a readiness problem

- **Actor/start:** System reader and administrator; one not-ready check and one independent failed provider plus
  normal resources.
- **Objective/steps:** Home → inspect instance/resources/listeners → Needs attention → Operations/Health →
  filter/check again → follow Fix link → return → SupportSummary print/download.
- **Expected:** Identity and current readiness remain consistent; a failed provider is named without hiding
  others; the support summary preserves useful context and redacts secrets.
- **Recovery:** No telemetry, unsupported host metric, refresh denied, check still running and unavailable Fix
  destination show distinct states.
- **Screens/modes:** Home, Operations/Health, SupportSummary; Monitor/Dashboards embed handoff,
  browser/TUI/Standard and redacted JSON/print.

### SYS-02 — Manage a disposable account through its lifecycle

- **Actor/start:** Instance administrator, dedicated target account and account-view counterpart; local OTP/mail
  sink.
- **Objective/steps:** Security/Users → create/find user → detail → change profile/password → add/end membership
  → enroll/verify offered factor → disable/enable or reset → inspect account/history and sign-in effect.
- **Expected:** Record identity persists; changes are visible to the target session; verification explains the
  factor and destination without exposing a secret; reset and disable outcomes are distinct.
- **Recovery:** Duplicate account, mismatched password, invalid/expired code, canceled disable,
  last-administration-path restriction and stale membership retain safe recovery.
- **Screens/modes:** UserAccount/List/Detail and VerifyEmail/Sms/Totp, factor dialogs; SH-01 owns full login
  completion, SYS-15 audits changes.

### SYS-03 — Grant a coherent capability and inspect its scope

- **Actor/start:** Prepared group operator, reader and target user; disposable group, registered bundle and row
  filter.
- **Objective/steps:** Access/Groups → create GroupDetail → add dated member → preview/apply grant →
  Capabilities inspect/change → RowRules bind → sign in as target → AccessReview as-of date → inspect raw
  ArtifactGroups separately as administrator.
- **Expected:** Bundle, app gate, row scope and effective dates are intelligible; preview matches result; raw
  artifact controls are distinct from registered delegation and do not become a bypass.
- **Recovery:** Stale grant revision, sealed/privileged grant, unsupported matrix rule, expired member, wrong
  filter binding and last-admin removal are refused for the right reason.
- **Screens/modes:** Access containers, Groups/GroupDetail, Capabilities, RowRules, ArtifactGroups/Detail,
  AccessReview; actual print/CSV evidence and TUI choices.

### SYS-04 — Configure sign-in and revoke a selected session

- **Actor/start:** Policy/provider operator, reader, local OIDC/OAuth/SAML fixture as applicable, two sessions
  of a disposable user.
- **Objective/steps:** SignInPolicy → inspect effective source → preview/save group rule →
  IdentityProviders/detail → configure/test mappings → enable → target sign-in → Sessions filter/select/revoke →
  inspect surviving session.
- **Expected:** On-disk versus editable rules are clear; provider test does not imply enabled sign-in; session
  revocation affects only the selected eligible sessions with a meaningful next login.
- **Recovery:** Invalid provider metadata, forbidden role mapping, canceled policy/revoke, expired session and
  unavailable provider preserve a usable administration path.
- **Screens/modes:** SignInPolicy, IdentityProviders/Detail, Sessions and account verification handoff; SH-01
  authentication, no external provider calls.
- **New-device handoff:** Use the captured AccountSignInEmail owned by SH-01 to enter the eligible session view,
  compare its bounded client/network summary, revoke that session and verify its next request. A notice is neither
  proof of unauthorized access nor evidence that revocation already occurred; preserve the other session.

### SYS-05 — Change a registered setting and localize its consumer

- **Actor/start:** Settings operator and reader; disposable preference/message/registered label with
  placeholders.
- **Objective/steps:** SettingsHome → choose permitted owner → InstanceSettings inspect effective/source →
  edit/save/reopen → visit consuming screen; Localization translate message/label → inspect consumer → raw rows
  mode as administrator → restore.
- **Expected:** Typed values, overrides, source and restart effects are clear; translation keeps placeholders;
  consumers show the intended value and language.
- **Recovery:** Invalid enum/number, forbidden/secret key, stale revision, missing translation and invalid
  placeholders preserve input and do not turn secrets into preferences.
- **Screens/modes:** SettingsHome, InstanceSettings, Localization normal/raw modes; cross-app consumer,
  browser/TUI/Standard and narrow forms.

### SYS-06 — Inspect, validate and reload configuration

- **Actor/start:** CONF_EDIT holder plus System reader; copied writable conf file and competing edit fixture.
- **Objective/steps:** Conf/View inspect origin → Edit choose file → change harmless value → Check/diff → Save →
  Changes review reload/restart classification → reload → View confirm → Logging tab → return.
- **Expected:** Merged source and file edit are distinct; checked changes match saved/reloaded values; pending
  restart is not presented as already active.
- **Recovery:** Malformed XML, read-only default conf, stale file hash, canceled save and failed independent
  reload item preserve original content and name the failure.
- **Screens/modes:** Conf/View/Edit/EditTui/Changes/Logging; MON-10 owns Logging edits; browser/TUI with
  supported Standard fallback, no live repo config writes.

### SYS-07 — Deploy and remove a disposable component

- **Actor/start:** COMPONENT_DEPLOY holder and missing-permission counterpart; local repository/component copy
  and isolated runtime.
- **Objective/steps:** Components → discover/preview → inspect refusals and conf diff → stage → deploy/load →
  load selected demo data → inspect loaded/pending → check/update local version → cancel then remove.
- **Expected:** Source/version, current versus next-start state and data-load effects are clear; removal names
  the component and dirty files; each independent failure remains visible.
- **Recovery:** Invalid component, unsatisfied dependency, dirty working copy, failed data file, permission loss
  and restart requirement retain safe continuation without claiming total success.
- **Screens/modes:** Components forms and dialogs; link to Tools artifacts/Dev AIDE only to verify the deployed
  fixture; no production/addon checkout touched.

### SYS-08 — Manage an instance definition and its actual lifecycle

- **Actor/start:** Instance administrator; disposable supported hosting backend with isolated ports/volumes, or
  explicitly unavailable backend.
- **Objective/steps:** Instances/InstanceList → create → InstanceDetail set environment/volume/link →
  provision/check/database/init as offered → start → inspect state → stop → remove with confirmation.
- **Expected:** Displayed lifecycle matches backend outcomes; instance/volume identity is clear and credentials
  stay protected; defined, provisioned and running are distinct.
- **Recovery:** Unsupported backend, conflicting port, failed phase, invalid environment, canceled removal and
  stale state identify the failed operation without damaging other definitions.
- **Screens/modes:** Instances/InstanceList and Instances/InstanceDetail, with Fleet, Environments, Hosts, Images,
  DatabaseHosts, Kinds and Rotation for its setup and follow-up; local backend required for successful lifecycle, with
  TUI/Standard administrative forms.

### SYS-09 — Inspect cache and thread pressure

- **Actor/start:** System reader and administrator; disposable cache entries and harmless active work.
- **Objective/steps:** Home resource/counter → CacheList → CacheElements → inspect selected entry → clear only
  review-owned cache where offered; ThreadList inspect work/stacks → Monitor link → return.
- **Expected:** Native/unavailable metrics are labelled; an empty cache is distinct from one whose entries are
  not exposed; thread count and executing artifact are coherent.
- **Recovery:** Unsupported stack/metric, finished thread, empty cache and refused clear leave navigation and
  independent readings useful.
- **Screens/modes:** Cache/CacheList/CacheElements, ThreadList, Home; TUI/Standard tables, narrow long-key/stack
  handling.

### SYS-10 — Configure a safe job and explain its partial run

- **Actor/start:** Registered job operator and reader; harmless fixture job with one successful and one failed
  item.
- **Objective/steps:** Operations/Jobs → JobDetail → inspect parameters/schedule/recipients → edit allowed field
  → Run → JobRuns → JobRunDetail → inspect failed item/log/trace → correct fixture and rerun according to
  operation semantics.
- **Expected:** Requested/running/completed/partial are distinct; run evidence ties to job and actor; masked
  parameters remain safe; successful items are not silently replayed.
- **Recovery:** Invalid registered parameter, locked/running job, denied run, canceled edit and failed item
  report the actual restriction rather than clearing locks.
- **Screens/modes:** Jobs/JobDetail/JobRuns/JobRunDetail and Monitor handoff; browser/TUI/Standard, row links
  and readable timestamps.

### SYS-11 — Set notification defaults and confirm the audience

- **Actor/start:** Topic operator and subscribed/non-audience user; a harmless job event and local capture where
  needed.
- **Objective/steps:** Topics → TopicDetail → inspect audience/defaults → change a permitted default → subscribe
  eligible user → trigger fixture event → inspect user notification and return.
- **Expected:** Defaults and individual subscription are distinguishable; messages reach the allowed audience
  and link to the correct event record.
- **Recovery:** Unregistered default, ineligible recipient, disabled channel and failed delivery remain visible
  without subscribing everyone.
- **Screens/modes:** Topics/TopicDetail and account topic controls; shared notification UI owns
  focus/navigation, TUI/Standard form branch.
- **Output handoff:** SH-12 owns the NotificationEmail/NotificationDigestEmail rendering and return links. Record
  the subscription/channel/digest settings that produced each captured message here, without counting a configured
  topic or queued event as delivered mail.

### SYS-12 — Inspect an import and retry only failed rows

- **Actor/start:** Import operator with a registered framework fixture target and mixed-result staged run;
  reader and denied counterpart.
- **Objective/steps:** Data/Imports → select run → ImportDetail → read row errors/results → download result →
  correct permitted failed rows using registered owner workflow → Retry → reopen evidence.
- **Expected:** Counts, row identities and operation key are clear; successful rows remain complete;
  duplicate/stale retries cannot reinterpret the original input.
- **Recovery:** Unreadable file, stale preview, restricted column, no failed rows and unavailable owner show
  explicit recovery limits.
- **Screens/modes:** Imports/ImportDetail and result/retry transitions; stage/apply is fixture or owner setup,
  not a fabricated System wizard; actual output files.

### SYS-13 — Export registered data and follow its retained delivery

- **Actor/start:** Export operator, scoped reader and outsider; registered format with allowed and restricted
  fields.
- **Objective/steps:** Data/DataExport → choose target/fields → review → download → Exports find retained
  result/delivery → inspect consumer link and bytes.
- **Expected:** Selected scope/columns match output; restricted fields are absent; a retained file/download and
  a delivered report are described accurately.
- **Recovery:** Invalid target/format, no rows, revoked grant, expired output and failed delivery preserve the
  reason and do not leak another user's bytes.
- **Screens/modes:** DataExport, Exports, Data landing Tools handoffs; DASH-09 owns report delivery;
  CSV/JSON/other actual offered formats, TUI/Standard.

### SYS-14 — Request a backup and understand its coverage

- **Actor/start:** Backup operator and reader; disposable database/runtime with supported backup job and local
  archive destination.
- **Objective/steps:** Data/Backups → inspect included/excluded stores → request backup → follow JobRun → reopen
  archive/coverage → inspect produced artifact and timestamp.
- **Expected:** A successful backup means an archive was written; exclusions and freshness are explicit; request
  acceptance is not completion or proof of restore.
- **Recovery:** Busy job, failed archive, unavailable destination, stale successful run and missing resource
  coverage leave named gaps.
- **Screens/modes:** Backups, JobDetail/RunDetail and archive metadata; no invented restore UI or real database
  restore.

### SYS-15 — Trace an administrative change to evidence

- **Actor/start:** Reader with audit authority and actor from SYS-02/03/05; known change reason and target.
- **Objective/steps:** Audit/Changes → filter actor/record/time → open registered destination → return; inspect
  raw log mode, AccessReview dated export, Sessions visits and VisitDetail.
- **Expected:** Actor, target, timestamp/reason and scope are clear; values are safely redacted; pagination
  filters do not follow a record link into unrelated destinations.
- **Recovery:** No matches, unavailable projection, missing record, hidden sensitive field and denied raw mode
  remain distinguishable.
- **Screens/modes:** AuditChanges, AccessReview, Sessions visits, VisitList/VisitDetail and account
  audit/history; print/CSV and TUI/Standard.

### SYS-16 — Maintain a local Git resource

- **Actor/start:** Resource administrator; separate bare/local remote and repository content, never the current
  worktree.
- **Objective/steps:** Resource/GitRepositoryList → create/detail → initialize/fetch → inspect
  refs/commits/update → export/import a disposable archive → inspect LFS/mirror/update retry controls where
  supported.
- **Expected:** Repository and resource URI identity remain visible; local changes and remote updates are
  distinguished; import/export bytes and update results match the selected repository.
- **Recovery:** Bad URL, unavailable remote, invalid archive, failed update, canceled maintenance and permission
  loss keep existing content and name the failure.
- **Screens/modes:** GitRepositoryList/Detail; no Dev AIDE source-workflow duplication, actual archive
  inspection and supported TUI/Standard forms.

### SYS-17 — Follow a print job to a confirmed outcome

- **Actor/start:** Print administrator and reader; local printer simulator/spool with a disposable document.
- **Objective/steps:** Print/PrinterList → discover/create → PrintJobList → Detail → inspect document →
  hold/release/send/cancel as eligible → inspect final spool/result.
- **Expected:** Printer, document and job state agree; held, sent and physically/simulated received are
  distinct; downloaded bytes are the selected job.
- **Recovery:** Unavailable printer, failed send, canceled action, denied document and stale job retain
  actionable status without claiming delivery.
- **Screens/modes:** Printer/PrinterList and PrintJob/List/Detail; local device required, TUI/Standard forms and
  actual print/download output.

### SYS-18 — End the intended terminal session

- **Actor/start:** Administrator and session owner; two disposable sessions created by SH-14, one still running.
- **Objective/steps:** System/TerminalSessions → inspect owner/state → open End confirmation → cancel → select
  correct session → end → verify owner feedback and other session survival.
- **Expected:** The confirmation names the session; ended status matches the user's terminal and other sessions
  remain intact.
- **Recovery:** Already-ended session, lost authority, stale list and unavailable host terminal have explicit
  feedback.
- **Screens/modes:** TerminalSessions/End dialog with SH-14 return; browser/TUI/Standard and keyboard
  confirmation.

### SYS-19 — Inspect certificates and web-protection state

- **Actor/start:** Administrator with local certificate/ACME fixture and synthetic ban/rule data; no public CA
  or real client traffic.
- **Objective/steps:** Certificates → inspect domain/expiry → set up/renew/stop/resume/remove a disposable
  certificate when backend permits → Security/WebProtection inspect counts/rules → lift one synthetic ban →
  follow edge-domain link.
- **Expected:** Domain, listener and certificate state are coherent; actions identify exactly what changes; a
  protection counter is not confused with an editable rule.
- **Recovery:** Missing ACME prerequisite, failed renewal, conflicting domain, canceled removal and stale/absent
  ban preserve truthful state and existing usable certificates.
- **Screens/modes:** Certificates and WebProtection; AI-19/20 edge context, browser/TUI/Standard; external
  success remains blocked without local fixture.

### SYS-20 — Reconcile raw controls, helper access and companion context

- **Actor/start:** Prepared readers/operators and outsider; known-good records from earlier waves.
- **Objective/steps:** Check each remaining inventory entry; follow Home → Monitor → Dashboards, data → Tools,
  integrations → Comms; pair selected direct helper/output refusals with valid holder requests.
- **Expected:** Every screen has a recorded activity or supporting inspection; raw administration never leaks
  through a mounted context; history returns to the originating record.
- **Recovery:** Revoked grant, optional backend missing, unregistered provider and invalid helper context remain
  named unresolved branches.
- **Screens/modes:** All core inventory and mode-specific helpers; no second full sweep, no addon application
  walkthrough.

## 5. Existing checks and future verification

This is an available-test map, not a results ledger. Read assertions and suite setup before selecting a run; static
shape checks, offline component fixtures, live browser walks and service rules provide different evidence.

| Component / suite | Existing specs | Coverage and limits |
| --- | --- | --- |
| tools / server | [AdminOverview](../runtime/components/base/tools/test/AdminOverview.spec.js), [SystemAttention](../runtime/components/base/tools/test/SystemAttention.spec.js), [HomeCatalog](../runtime/components/base/tools/test/HomeCatalog.spec.js) | Home availability/attention/navigation, cache/thread wording and job link context; does not exercise full administration. |
| tools / server | [AdminMounts](../runtime/components/base/tools/test/AdminMounts.spec.js) | Raw-transition and mounted-context boundaries, including static guards; no addon runtime is needed. |
| tools / server | [ConfView](../runtime/components/base/tools/test/ConfView.spec.js), [ConfReload](../runtime/components/base/tools/test/ConfReload.spec.js), [HostedInstances](../runtime/components/base/tools/test/HostedInstances.spec.js) | Conf origins/masking/check/stale saves and reload/hosted-instance rules; backend capability remains a live prerequisite. |
| framework / server | [AdminAccessServices](../runtime/components/base/framework/test/AdminAccessServices.spec.js), [AdminAccessCapabilities](../runtime/components/base/framework/test/AdminAccessCapabilities.spec.js), [AdminIdentityProviders](../runtime/components/base/framework/test/AdminIdentityProviders.spec.js), [AdminSessions](../runtime/components/base/framework/test/AdminSessions.spec.js) | Grant/revision/last-admin, capability/row binding, provider mapping and session rules; not UI access walks. |
| framework / server | [AdminConfiguration](../runtime/components/base/framework/test/AdminConfiguration.spec.js), [AdminReadiness](../runtime/components/base/framework/test/AdminReadiness.spec.js), [AdminJobs](../runtime/components/base/framework/test/AdminJobs.spec.js) | Typed preferences/translations, readiness/redacted summary, registered job parameters/partial outcomes and topic audiences. |
| framework / server | [AdminDataTransfer](../runtime/components/base/framework/test/AdminDataTransfer.spec.js), [AdminDataExport](../runtime/components/base/framework/test/AdminDataExport.spec.js), [AdminBackups](../runtime/components/base/framework/test/AdminBackups.spec.js), [AdminAuditChanges](../runtime/components/base/framework/test/AdminAuditChanges.spec.js) | Mixed-row retries/idempotency, registered scoped output, archive-based success and redacted audit rules. |

Future acceptance candidates: **SystemAccessWalk** (SYS-02–04, 15), **SystemConfigurationWalk** (SYS-05–08),
**SystemOperationsWalk** (SYS-01, 09–11), **SystemDataWalk** (SYS-12–14) and **SystemResourcesWalk** (SYS-16–19).
These are UI journeys over existing rules. A later System sweep must be shared by all three System plans, not one
sweep per document. No existing test name here establishes successful container, printer, ACME or identity-provider
execution; those branches need local backend evidence or a blocked disposition.

Follow the [verification policy](ui-review-shared-gallery.md#verification-policy-for-later-execution). Proposed
walk names above describe future acceptance work, not tests created or authorized to run by this document. Keep
focused bug/rule checks in regression and reuse existing sweeps/walks instead of adding duplicates.

## 6. Coverage, findings and running record

### Activity coverage ledger

Statuses: **pending**, **inspected**, **exercised** (pass/fail/partial named separately), **blocked**, **deferred**
(with reason). Existing tests and the archived review do not advance these statuses. Every branch is pending until
execution records it; “exercised” never implies that all modes and recovery paths passed.

| Activity | Status | Outcome / remaining branches | Evidence / findings |
| --- | --- | --- | --- |
| SYS-01 — Assess the instance and follow a readiness problem | exercised — partial | Home, Health state filter/recheck, Support summary and telemetry Fix handoff exercised. SYS-B02 observed an explicit local-exporter loss warning after recheck and confirmed dropped records on Sources. Other filters, refresh authorization, JSON/print and remaining modes pending. | Batch 01: 01–04, 12; SYS-B02; SYS-UI-001 |
| SYS-02 — Manage a disposable account through its lifecycle | exercised — partial | Username no-match search/reset, existing John Doe detail and membership navigation exercised. Disposable creation, validation, lifecycle, audit/password/factor actions, scoped actors and remaining modes pending. | Batch 01: 05–07 |
| SYS-03 — Grant a coherent capability and inspect its scope | exercised — partial | Existing ADMIN group, sealed-grant explanation, members and return to John Doe inspected. Groups search, capabilities comparison, row rules, every grant/change/refusal branch and remaining modes pending. | Batch 01: 06–07 |
| SYS-04 — Configure sign-in and revoke a selected session | pending | All steps, modes and recovery branches pending. | — |
| SYS-05 — Change a registered setting and localize its consumer | exercised — partial | Effective value/source table inspected; Instance name draft closed without Save and blank value verified on return/reopen. Save, invalid/stale input, consumer localization, restricted actors and remaining modes pending. | Batch 01: 08–09, 13 |
| SYS-06 — Inspect, validate and reload configuration | exercised — partial | Settings → Open the conf, merged-source search for telemetry_enabled and Runtime source entry exercised; editor opened without edits. SYS-B02 verified the accessible filter name, no-match state and Clear recovery. Validation/save/reload/conflict/Logging handoff, full keyboard walk and other modes pending. | Batch 01: 10; SYS-B02; SYS-UI-002 |
| SYS-07 — Deploy and remove a disposable component | inspected — partial | Loaded six-component inventory and empty deployment history inspected. Fetch/preview/deploy/load/remove, failure recovery, refusal and remaining modes pending. | Batch 01: 11 |
| SYS-08 — Manage an instance definition and its actual lifecycle | pending | All steps, modes and recovery branches pending. | — |
| SYS-09 — Inspect cache and thread pressure | pending | All steps, modes and recovery branches pending. | — |
| SYS-10 — Configure a safe job and explain its partial run | pending | All steps, modes and recovery branches pending. | — |
| SYS-11 — Set notification defaults and confirm the audience | pending | All steps, modes and recovery branches pending. | — |
| SYS-12 — Inspect an import and retry only failed rows | pending | All steps, modes and recovery branches pending. | — |
| SYS-13 — Export registered data and follow its retained delivery | pending | All steps, modes and recovery branches pending. | — |
| SYS-14 — Request a backup and understand its coverage | pending | All steps, modes and recovery branches pending. | — |
| SYS-15 — Trace an administrative change to evidence | pending | All steps, modes and recovery branches pending. | — |
| SYS-16 — Maintain a local Git resource | pending | All steps, modes and recovery branches pending. | — |
| SYS-17 — Follow a print job to a confirmed outcome | pending | All steps, modes and recovery branches pending. | — |
| SYS-18 — End the intended terminal session | pending | All steps, modes and recovery branches pending. | — |
| SYS-19 — Inspect certificates and web-protection state | pending | All steps, modes and recovery branches pending. | — |
| SYS-20 — Reconcile raw controls, helper access and companion context | pending | All steps, modes and recovery branches pending. | — |

### Findings register

Use `SYS-UI-001` onward. P1 blocks work or essential understanding; P2 causes substantial friction or misleading
presentation; P3 is a localized refinement. Label evidence as observed behavior, static suspicion or design judgment.

| ID / severity | Actor, activity step, route, mode / viewport | Reproduction and impact | Evidence / owning layer | State / correction / retest |
| --- | --- | --- | --- | --- |
| SYS-UI-001 / P2 / observed | `john.doe`, SYS-01, Home → Operations/Health → Monitor/Sources, Material light; initial Home 1280×720, Health/Sources 1920×1080 | Health says “Telemetry is off or not searchable, so health is unknown,” including after recheck more than five minutes after startup. Its Fix destination shows recent local telemetry, 14,686 exported / zero failed or dropped; Logs searches and trace navigation work. Sources also lists an older startup/loading instance with no telemetry in five minutes. The aggregate explanation does not distinguish a stale publisher from an unavailable pipeline and directs the operator toward the wrong diagnosis. | System evidence 01–04, 10, 12; Monitor batch 01 evidence 03–10. Primary owner: readiness wording/aggregation in `framework/service/org/moqui/admin/Readiness.js` (`admin.telemetry`); Monitor owns source diagnosis. Static support: any `unknown` source, or an empty source list, maps to the same message. Exact triggering source remains unproven. | Fixed and retested. Readiness previously treated the local exporter’s explicit “health unknown” suffix as export failure. It now reads the local and configured remote states from get#SourceStatus. AdminReadiness checks disabled, unavailable search, waiting, loss, success, mixed exporters and the restricted reader’s summary. On the local runtime, Sources showed recent and stale publishers together. Health returned Ready after Check again, Support showed “Local telemetry has been exported”, and Home listed no telemetry warning. A second runtime recorded 136 dropped records with only active publishers and 308 with fresh and stale publishers. Home, Health after recheck, and Support all reported local exporter loss in both cases. **2026-11-25:** Passed the SYS-B02 browser recheck below. |
| SYS-UI-002 / P3 / observed accessibility | `john.doe`, SYS-06, `Conf/View`, Material light 1920×1080 | The visually identifiable “Search settings” field is exposed in the accessibility tree as a generated `f_…` identifier when empty and “Clear” after entering a query. Its purpose is not represented by a stable accessible name, making this configuration filter hard to identify through assistive navigation. Filtering itself succeeds. | Batch 01 evidence 10; live empty/full AX inspection. `tools/screen/System/Conf/View.qvue:33` supplies placeholder text without an explicit label/aria-label. This is accessibility-tree evidence, not a completed screen-reader or keyboard walk. | Fixed and retested. Conf/View supplies an explicit accessible name on the native input. Chrome found the Search settings textbox empty and populated. A nonmatching query hid the settings. The independently named Clear button restored the empty textbox and all 51 previously visible entries. **2026-11-25:** Passed the SYS-B02 browser recheck below. |

### Bounded batch record

For each later batch record:

- Intent, activities and branch limits; runtime/build/components, identities/grants, fixture IDs and dates.
- Observed steps and outcomes; evidence filenames, console/network/output observations and findings.
- Workarounds and blocked/deferred branches, with prerequisites and follow-up.
- In refinement only: decisions, owning files/components, correction, focused verification commands/results and retest.
- Temporary settings/grants/files/jobs restored, remaining cleanup and companion handoffs.

### Batch 01 — Administration orientation and read-only handoffs — 2026-11-16

**Intent and bounds.** Start SYS-01/02/03/05/06/07 with visible navigation, existing records, empty-search recovery
and cancellation. Review pass only: no application changes, configuration/grant saves, account lifecycle actions,
jobs, component deployment or automated tests. The separate review → findings triage → refinement sequence remains
in force.

**Runtime and actor.** Current checkout `83cf05b3b6b069812a6ac2802d95f918db6a9802`, freshly built debug binary
SHA-256 `f799a82c681a7c3c9a7973cf5e030a5d1ebf41fa8935280f0d527dbf61bb2004`; fresh embedded PostgreSQL at
`/tmp/mr-ui-four-apps-zf5098dh`, started 2026-11-16 23:49 UTC. Six loaded components: framework, webroot,
basic-comms, tools, dev-aide and example; no addons/test components. Telemetry enabled with sample ratio 1 before
startup; scheduled jobs disabled, no external mail/media/provider fixture. Existing seed `john.doe` / `EX_JOHN_DOE`
with ADMIN + ADMIN_ADV supplied the baseline; none of the proposed restricted identities was prepared or exercised.
The root reviewer coordinated runtime lifecycle and the build; this worker ran no build or test commands.

Use `http://127.0.0.1:18872/qapps/system` for this browser session. An initial shared-cookie return-URL collision
with the parallel review on `localhost` was resolved by hostname isolation and is a harness artifact, not a finding.
Material light, default/comfortable presentation; initial Home was captured at 1280×720 (1265×720 content), then
the agreed desktop viewport was set and measured at 1920×1080 for the rest. This is not responsive-mode coverage.
Dark, 390px, deliberate 1280px breakpoint checks, keyboard, Standard, TUI and output branches remain pending.

**Observed walk.**

1. Home rendered readiness, integration/listener states, six loaded components and administration links. Missing
   platform measurements were represented by em dashes; Activity panels had no data in the initial time range.
   Health showed separately explained warnings for the deliberately unconfigured mail and backup fixtures.
2. Health's State = Unavailable filter returned the telemetry row. Check again re-evaluated that provider and
   replaced the state-filter URL with `providerKey=admin.telemetry`. Support summary rendered readiness, protocol
   and effective-setting sections with Print/Download JSON affordances; those output actions were not executed.
3. Security's user list showed four seed/system accounts. Find Options → Username `ui-review-no-user` → Find
   displayed “No results found,” the active filter and Reset filters to default. Reset restored John Doe; opening
   the account showed identity, ADMIN/ADMIN_ADV memberships, factors and login history. ADMIN opened its group
   with one current member and sealed grants; the John Doe member link returned to the correct account.
4. Settings showed effective values, origins and when changes take effect. Entered `UI review cancelled name`
   in the Instance name dialog, then used Close dialog without Save. After leaving and returning, the table still
   said Not set and reopening showed an empty Value. No setting was persisted. Open the conf led to the merged
   tree; searching `telemetry_enabled` exposed Runtime:12 and the resolved telemetry node (`true`, sample ratio 1).
   Runtime opened its source editor with Review and save disabled; no edit/save/reload was attempted.
5. Components listed exactly the six expected loaded components and an explicit empty deployment-history message.
   Deploy and Load Data controls were inspected only.
6. The bounded [Monitor batch](ui-review-system-monitoring.md#6-coverage-findings-and-running-record) used the same
   session to recover an invalid log query, follow a real log through context → trace → logs, and inspect Sources.
   Returning to Health at 17:54 local still produced the unavailable explanation; its Fix link reached Sources
   with recent searchable telemetry. SYS-UI-001 owns this disagreement; it does not establish a telemetry outage.

**Evidence.** Ignored local directory `plans/ui-refinement-evidence/system/batch-01/`: `runtime-metadata.json`,
`UiReviewConf.xml`, `preflight.md` (supporting source notes only), and the numbered AX text / JPEG captures below.
These files are local evidence, not committed artifacts; filenames identify precisely what was observed.

| Evidence prefix | Captured outcome |
| --- | --- |
| `01-home` | Initial Home, including attention and integration states |
| `02-health-unavailable`, `03-health-recheck` | Filtered unavailable row and provider recheck |
| `04-support-summary` | Readiness/protocol/effective-settings summary; outputs remain pending |
| `05-users-empty`, `06-user-detail`, `07-admin-group` | Empty search, recovered detail and member/grant context |
| `08-settings`, `09-settings-cancel`, `13-settings-cancel-verified` | Provenance, unsaved draft and verified cancellation after return |
| `10-conf-telemetry` | Filtered merged configuration and accessible field name |
| `11-components` | Loaded inventory and deployment empty state |
| `12-health-after-telemetry` | Repeated unavailable explanation after current Monitor data was observed |

Monitor evidence is in `plans/ui-refinement-evidence/system-monitoring/batch-01/`; `09-sources` and
`10-health-fix-sources` support the cross-plan finding. Screenshots are selected viewport captures, not proof that
every off-screen row was visually reviewed; AX text records the additional displayed controls/data.

**Pending and cleanup.** Every unlisted branch remains pending, especially restricted actors, successful writes and
their consumers, failures/refusals, validation, all remaining modes and outputs. No finding was fixed or retested.
No grants or global preferences were intentionally changed; the browser may retain normal find/page-size/query
history from navigation. The unsaved settings dialog was closed, viewport override reset and session left signed
in for the coordinated batch. Runtime/database/evidence retention and final shutdown remain with the root reviewer.

**Coordinated shutdown.** After the four browser batches, root stopped the shared server with SIGINT at
2026-11-17 00:02:38 UTC (Nov 16 locally), exit 0. Server/HTTP listener and runtime PostgreSQL processes/PID file
were absent; curl cookie file removed. Browser viewport overrides were reset. Session logout was not established;
server shutdown ended availability. Scratch runtime/database and ignored local evidence are retained. See
`runtime-cleanup.json` in this batch's evidence directory.

### SYS-B02 — Post-cleanup recheck, 2026-11-25

The coordinator walked both findings as `john.doe` in the built-in browser, Material light at 1920×1080, on the disposable November 25 review instance (release binary, checkout `6d8bb49f8a7c`). The following observations are from that session.

| Finding | Action and observed result |
| --- | --- |
| SYS-UI-001 / SYS-01 | At 05:35 CST, Check again on Operations → Health warned “The local exporter recorded telemetry loss”. Monitor → Sources showed 14,630 queued, 1,349 exported, **4,267 dropped** and zero failed. Both a recent local publisher and a separate stale publisher were present. The warning correctly described loss on a runtime that was actively exporting. The counters were sampled together and do not establish the fate of every queued record. |
| SYS-UI-002 / SYS-06 | On Conf → View, the empty filter exposed the accessible name “Search settings”. Entering `zz-review-no-such-attribute` produced the no-match state while retaining that textbox name. The separate Clear button restored the settings sections. No conf value was changed. |

Both historical findings passed in the observed states. The active zero-loss, disabled-telemetry, restricted-reader and alternate-mode branches retain their prior coverage. The two fixture roles available to companion reviewers are `sec.view.only` (SEC_VIEW_GROUP) and `sec.none.only` (SEC_NONE_GROUP). The System recheck used the administrator. The source comparison belongs to [Monitor](ui-review-system-monitoring.md); this register owns the readiness finding.

## 7. Decisions

- `SYS` owns core System screens, including Certificates and Security/WebProtection; edge configuration links back
  from the AI/integrations plan. Monitoring and AI/integration subtrees are assigned there, not omitted.
- Existing ADMIN access is an administration baseline only. Restricted actors need separately recorded grants.
- System import lists inspect registered runs; staging belongs to the registered consumer or a disposable fixture.
  Do not invent a System import wizard, backup restore screen or future fleet-management workflow.

- Preparing or auditing these plans changes documentation only. Application/API changes, test runs, runtime walks
  and fixture creation belong to separate review/refinement execution.
- Current code, mounts and grants take precedence over older plans. Historical outcomes, permissions and exceptions
  are not inherited. Preserve routes, stored preferences and public contracts in any later refinement.
- Finish a review with explicit branch outcomes and a candid unresolved ledger; missing prerequisites do not become
  passes. Building absent features is outside this review.
